require "minitest/autorun"
require "yaml"

class CiWorkflowsTest < Minitest::Test
  ROOT = File.expand_path("../..", __dir__)
  CI = YAML.load_file(File.join(ROOT, ".github/workflows/ci.yml"))
  QUALITY = YAML.load_file(File.join(ROOT, ".github/workflows/_quality.yml"))
  CONTAINER = YAML.load_file(File.join(ROOT, ".github/workflows/_container.yml"))

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
end
