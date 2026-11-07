require "minitest/autorun"
require "yaml"

class CiWorkflowsTest < Minitest::Test
  ROOT = File.expand_path("../..", __dir__)
  CI = YAML.load_file(File.join(ROOT, ".github/workflows/ci.yml"))
  QUALITY = YAML.load_file(File.join(ROOT, ".github/workflows/_quality.yml"))
  CONTAINER = YAML.load_file(File.join(ROOT, ".github/workflows/_container.yml"))
  CACHE = YAML.load_file(File.join(ROOT, ".github/workflows/build-cache.yml"))

  def step(workflow, job, name)
    workflow.fetch("jobs").fetch(job).fetch("steps").find { |item| item["name"] == name }
  end

  def selected?(area, path)
    filter = step(CI, "changes", "Detect affected checks")
    patterns = YAML.safe_load(filter.fetch("with").fetch("filters")).fetch(area)
    positive, negative = patterns.partition { |pattern| !pattern.start_with?("!") }
    positive.any? { |pattern| File.fnmatch?(pattern, path, File::FNM_DOTMATCH) } &&
      negative.none? { |pattern| File.fnmatch?(pattern.delete_prefix("!"), path, File::FNM_DOTMATCH) }
  end

  def test_frontend_only_has_quality_and_a_deployable_image_without_backend_tests
    %w[frontend/src/views/accounts/AccountsView.vue frontend/src/style.css frontend/public/logo.png
       frontend/tests/account-models.test.mjs frontend/package.json frontend/pnpm-lock.yaml].each do |path|
      assert selected?("frontend", path), path
      assert selected?("container", path), path
      refute selected?("backend", path), path
    end
    assert_equal true, CI["jobs"]["container"]["with"]["save_image"]
    assert_equal "${{ needs.changes.outputs.backend == 'true' }}", CI["jobs"]["quality"]["with"]["run_backend"]
  end

  def test_planner_and_build_changes_cannot_bypass_their_checks
    %w[release/ci_plan.py release/build_ci_backend.sh deploy/Dockerfile .github/workflows/ci.yml
       .github/workflows/_quality.yml .github/workflows/_container.yml].each do |path|
      assert selected?("backend", path), path
      assert selected?("container", path), path
    end
    %w[release/tests/test_ci_plan.py release/tests/test_ci_build.py release/tests/ci_workflows_test.rb].each do |path|
      assert selected?("deployment", path), path
    end
    assert_equal %w[changes container deployment-tools quality validated], CI["jobs"].keys.sort
    %w[backend frontend container].each do |area|
      assert selected?(area, 'release/toolchains.rb'), area
      assert selected?(area, 'deploy/Dockerfile'), area
    end
  end

  def test_other_quality_callers_default_to_full_and_cargo_receives_data_not_shell_code
    trigger = QUALITY["on"] || QUALITY[true]
    inputs = trigger["workflow_call"]["inputs"]
    assert_equal "[]", inputs["backend_packages"]["default"]
    assert_equal true, inputs["run_redis_acl"]["default"]
    ["Rust clippy", "Build Rust tests once"].each do |name|
      command = step(QUALITY, "backend", name)
      assert_includes command["run"], 'release/ci_plan.py cargo --packages "$BACKEND_PACKAGES"'
      refute_includes command["run"], "${{ inputs.backend_packages }}"
    end
    acl = step(QUALITY, "backend", "Redis named-user ACL regression")
    assert_equal "inputs.run_redis_acl", acl["if"]
    assert_includes acl["run"], "ACL SETUSER default off"
    assert_includes acl["run"], "package(=gateway-store) & test(/^redis::/)"
    assert_includes step(QUALITY, "backend", "Build Rust tests once")["run"], "--lib --test main --locked"
    assert_includes step(QUALITY, "backend", "Rust tests")["run"], "--binaries-metadata"
    assert_match(/^mod redis;/, File.read(File.join(ROOT, "backend/crates/gateway-store/tests/main.rs")))
  end

  def test_runtime_builds_source_for_dependency_changes_and_reuses_validated_assets_otherwise
    %w[deploy/Dockerfile backend/Cargo.lock backend/crates/gateway-core/Cargo.toml
       backend/apps/gateway/build.rs frontend/pnpm-lock.yaml frontend/.npmrc].each do |path|
      assert selected?("source_container", path), path
    end
    refute selected?("source_container", "backend/crates/gateway-core/src/lib.rs")
    refute selected?("container", "frontend/README.md")
    image = step(CONTAINER, "container", "Build runtime image")["with"]
    assert_equal "${{ inputs.source_build && 'runtime' || 'runtime-prebuilt' }}", image["target"]
    assert_equal "linux/amd64", image["platforms"]
    assert_includes image["no-cache-filters"], "runtime-base"
    assert_includes image["labels"], "org.opencontainers.image.revision=${{ inputs.ref }}"
    assert_includes step(CONTAINER, "container", "Require prebuilt inputs")["run"], 'test "$BINARY_RESULT" = success'
    refute step(CONTAINER, "container", "Verify runtime image").key?("if")
    assert step(CONTAINER, "container", "Save the verified image")
    binary = CONTAINER["jobs"]["binary"]["steps"].find { |item| item["uses"] == "./.github/actions/build-binary" }
    assert_equal "${{ inputs.ref }}", binary["env"]["CPR_GIT_SHA"]
    action = YAML.load_file(File.join(ROOT, ".github/actions/build-binary/action.yml"))
    build = action["runs"]["steps"].find { |item| item["name"] == "Build binary" }
    assert_includes build["run"], "cargo build --release --locked --timings"
    assert_includes build["run"], 'test -s "target/$BUILD_TARGET/release/$BUILD_BINARY"'
    assert_includes build["run"], 'test -x "target/$BUILD_TARGET/release/$BUILD_BINARY"'
    assert_includes build["run"], 'export CPR_BUILD_TYPE="${CPR_BUILD_TYPE:-release}"'
    assert_includes build["run"], 'export CPR_BUILD_TIME="${CPR_BUILD_TIME:-$(date -u +%Y-%m-%dT%H:%M:%SZ)}"'
    refute build.key?("if"), "Cache misses still build current source"
  end

  def test_merge_reuse_and_main_gate_are_preserved
    assert step(CI, "changes", "Reuse matching PR validation after merge")
    %w[quality container deployment-tools].each do |job|
      assert_includes CI["jobs"][job]["if"], "needs.changes.outputs.reused != 'true'"
    end
    assert_equal %w[changes quality container deployment-tools], CI["jobs"]["validated"]["needs"]
    assert_equal "always()", CI["jobs"]["validated"]["if"]
  end

  def test_cache_maintenance_is_trusted_main_only_and_never_a_validation_gate
    triggers = CACHE["on"] || CACHE[true]
    assert_equal %w[workflow_dispatch workflow_run], triggers.keys.sort
    assert_equal ["CI"], triggers["workflow_run"]["workflows"]
    assert_equal ["main"], triggers["workflow_run"]["branches"]
    assert_equal({"contents" => "read"}, CACHE["permissions"])
    assert_equal false, CACHE["concurrency"]["cancel-in-progress"]
    CACHE["jobs"].each do |name, job|
      ["github.event_name == 'workflow_dispatch' && github.ref == 'refs/heads/main'",
       "github.event.workflow_run.event == 'push'",
       "github.event.workflow_run.conclusion == 'success'",
       "github.event.workflow_run.head_branch == 'main'",
       "github.event.workflow_run.head_repository.full_name == github.repository",
       "github.event.workflow_run.path == '.github/workflows/ci.yml'"].each do |guard|
        assert_includes job["if"], guard
      end
      checkout = step(CACHE, name, "Checkout validated source")["with"]
      assert_equal "${{ github.event.workflow_run.head_sha || github.sha }}", checkout["ref"]
      assert_equal false, checkout["persist-credentials"]
      refute job.key?("services"), "Warming dependencies must not start integration services"
    end
    refute_includes CI["jobs"].keys, "build-cache"
    CACHE["jobs"].values.flat_map { |job| job["steps"] }.each do |entry|
      refute_match(/cargo (?:test|nextest run)|verified_image\.py/, entry["run"].to_s)
    end
  end

  def test_quality_cache_fingerprint_and_compile_only_miss_path_match_real_ci
    assert_equal "1", QUALITY["jobs"]["backend"]["env"]["CARGO_PROFILE_DEV_DEBUG"]
    assert_equal QUALITY["jobs"]["backend"]["env"], CACHE["jobs"]["quality"]["env"]
    baseline = step(QUALITY, "backend", "Cache Rust")["with"]
    lookup = step(CACHE, "quality", "Look up quality cache")["with"]
    restore = step(CACHE, "quality", "Restore and save missing quality cache")
    %w[workspaces shared-key cache-bin].each do |key|
      assert_equal baseline[key], lookup[key]
      assert_equal baseline[key], restore["with"][key]
    end
    assert_equal true, lookup["lookup-only"]
    assert_equal false, lookup["save-if"]
    assert_equal "steps.cache.outputs.cache-hit != 'true'", restore["if"]
    compile = step(CACHE, "quality", "Compile missing quality dependencies")
    assert_equal restore["if"], compile["if"]
    assert_includes compile["run"], "--workspace --lib --test main --locked --list-type binaries-only"
    assert_includes baseline["save-if"], "github.event.pull_request.head.repo.full_name == github.repository"
  end

  def test_release_cache_hit_skips_only_cache_maintenance_not_real_binary_builds
    lookup = step(CACHE, "release", "Look up release cache")["with"]
    build = step(CACHE, "release", "Compile missing release dependencies")
    assert_equal true, lookup["lookup-only"]
    assert_equal false, lookup["save-if"]
    assert_equal "steps.cache.outputs.cache-hit != 'true'", build["if"]
    assert_equal "./.github/actions/build-binary", build["uses"]
    assert_equal "release-#{build['with']['target']}-#{build['with']['rust-image']}", lookup["shared-key"]
    assert_equal "${{ github.event.workflow_run.head_sha || github.sha }}", build["env"]["CPR_GIT_SHA"]
    action = YAML.load_file(File.join(ROOT, ".github/actions/build-binary/action.yml"))
    actual_build = action["runs"]["steps"].find { |item| item["name"] == "Build binary" }
    refute actual_build.key?("if"), "Verified images still build their current source and metadata"
  end
end
