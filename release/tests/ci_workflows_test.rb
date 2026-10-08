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
    YAML.safe_load(filter.fetch("with").fetch("filters")).fetch(area).any? do |pattern|
      File.fnmatch?(pattern, path, File::FNM_DOTMATCH)
    end
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
  end

  def test_other_quality_callers_default_to_full_and_cargo_receives_data_not_shell_code
    trigger = QUALITY["on"] || QUALITY[true]
    inputs = trigger["workflow_call"]["inputs"]
    assert_equal "[]", inputs["backend_packages"]["default"]
    assert_equal true, inputs["run_redis_acl"]["default"]
    %w[Rust\ clippy Rust\ tests].each do |name|
      command = step(QUALITY, "backend", name)
      assert_includes command["run"], 'release/ci_plan.py cargo --packages "$BACKEND_PACKAGES"'
      refute_includes command["run"], "${{ inputs.backend_packages }}"
    end
    acl = step(QUALITY, "backend", "Redis named-user ACL regression")
    assert_equal "inputs.run_redis_acl", acl["if"]
    assert_includes acl["run"], "ACL SETUSER default off"
    assert_includes acl["run"], "--lib --test main --locked redis::"
    assert_match(/^mod redis;/, File.read(File.join(ROOT, "backend/crates/gateway-store/tests/main.rs")))
  end

  def test_runtime_uses_current_binary_and_full_frontend_not_an_old_image
    build = step(CONTAINER, "container", "Build current backend with cached compilation")
    assert_equal "${{ inputs.ref }}", build["env"]["CPR_GIT_SHA"]
    assert_equal "bash release/build_ci_backend.sh", build["run"]
    image = step(CONTAINER, "container", "Build runtime image")["with"]
    assert_equal "runtime-ci", image["target"]
    assert_equal "linux/amd64", image["platforms"]
    assert_includes image["labels"], "org.opencontainers.image.revision=${{ inputs.ref }}"
    dockerfile = File.read(File.join(ROOT, "deploy/Dockerfile"))
    assert_match(/FROM rust:[^\n]+@sha256:[a-f0-9]{64} AS backend-toolchain/, dockerfile)
    assert_includes dockerfile, "FROM backend-toolchain AS backend-builder"
    stage = dockerfile.split("FROM runtime-base AS runtime-ci", 2).last.split("FROM runtime-base AS runtime-prebuilt", 2).first
    assert_includes stage, "COPY --from=frontend-builder"
    assert_includes stage, "${CPR_PREBUILT_DIR}/linux-amd64/codex-proxy-rs"
    assert_equal "backend-toolchain", step(CONTAINER, "container", "Load pinned backend toolchain")["with"]["target"]
    backend = dockerfile.split("FROM backend-toolchain AS backend-builder", 2).last.split(/^FROM /, 2).first
    copied = backend.lines.grep(/^COPY /).flat_map { |line| line.split[1...-1] }
    assert_equal %w[backend/Cargo.toml backend/Cargo.lock backend/apps backend/crates backend/migrations release/version.yaml], copied,
                 "Update cached CI source staging together with backend-builder"
    assert_equal 1, backend.lines.grep(/^RUN /).length,
                 "Additional toolchain preparation must also reach the cached CI build"
  end

  def test_cache_miss_still_builds_and_verifies_and_scheduled_runtime_stays_fresh
    cache = step(CONTAINER, "container", "Cache backend compilation")["with"]
    assert_equal "${{ runner.temp }}/cpr-ci-cargo", cache["path"]
    assert_includes cache["key"], "deploy/Dockerfile"
    refute step(CONTAINER, "container", "Build current backend with cached compilation").key?("if")
    refute step(CONTAINER, "container", "Verify runtime image").key?("if")
    policy = step(CONTAINER, "container", "Select runtime cache policy")["run"]
    assert_includes policy, "echo 'runtime-base'"
    assert_includes policy, "echo 'runtime-ci'"
    security = YAML.load_file(File.join(ROOT, ".github/workflows/security-scan.yml"))
    assert_equal true, security["jobs"]["container-security"]["with"]["fresh_runtime"]
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
