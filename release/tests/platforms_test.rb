require "digest"
require "fileutils"
require "json"
require "minitest/autorun"
require "open3"
require "tmpdir"
require "yaml"

class ReleasePlatformsTest < Minitest::Test
  ROOT = File.expand_path("../..", __dir__)
  WORKFLOW = YAML.load_file(File.join(ROOT, ".github/workflows/release.yml"))
  JOBS = WORKFLOW.fetch("jobs")
  DEFAULTS = YAML.load_file(File.join(ROOT, "release/platforms.yaml")).fetch("platforms")
  RESTORED = YAML.safe_load(
    File.read(File.join(ROOT, "deploy/README.md")).match(/```yaml\n(  - os: linux.*?)```/m)[1]
  )

  def script(job, step_name, delimiter)
    step = JOBS.fetch(job).fetch("steps").find { |item| item["name"] == step_name }
    step.fetch("run").match(/<<'#{delimiter}'[^\n]*\n(.*?)^#{delimiter}$/m)[1]
  end

  def fixture(platforms)
    Dir.mktmpdir("cpr-platform-test-") do |dir|
      FileUtils.mkdir_p(File.join(dir, "release"))
      File.write(File.join(dir, "release/platforms.yaml"), { "platforms" => platforms }.to_yaml)
      File.write(File.join(dir, "release/version.yaml"), { "version" => "1.0.0" }.to_yaml)
      File.write(File.join(dir, "release/notes.md"), "# v1.0.0\n\nSynthetic release notes.\n")
      FileUtils.mkdir_p(File.join(dir, "deploy"))
      %w[config.example.yaml compose.yaml].each do |name|
        FileUtils.cp(File.join(ROOT, "deploy", name), File.join(dir, "deploy", name))
      end
      _, error, status = Open3.capture3("git", "init", "-q", dir)
      assert status.success?, error
      _, error, status = Open3.capture3(
        "git", "-c", "user.name=Test", "-c", "user.email=test@example.invalid",
        "-c", "commit.gpgsign=false", "commit", "--allow-empty", "-qm", "fixture", chdir: dir
      )
      assert status.success?, error
      yield dir
    end
  end

  def metadata(dir)
    stdout, stderr, status = Open3.capture3(
      { "RELEASE_TAG" => "v1.0.0" }, "ruby", "-e",
      script("release-info", "Read release metadata", "RUBY"), chdir: dir
    )
    [stdout.lines.map { |line| line.strip.split("=", 2) }.to_h, stderr, status]
  end

  def test_default_platform_and_optional_job_gate
    assert_equal ["linux/amd64"], DEFAULTS.map { |platform| platform["dockerPlatform"] }
    assert_equal "${{ steps.version.outputs.has_non_docker }}",
                 JOBS["release-info"]["outputs"]["has_non_docker"]
    assert_equal "${{ needs.release-info.outputs.has_non_docker == 'true' }}",
                 JOBS["build-binary"]["if"]
  end

  def test_real_metadata_and_packages_for_single_and_restored_platforms
    [DEFAULTS, DEFAULTS + RESTORED.first(1), DEFAULTS + RESTORED].each do |platforms|
      fixture(platforms) do |dir|
        output, error, status = metadata(dir)
        assert status.success?, error
        native = platforms.reject { |platform| platform["docker"] }
        assert_equal (!native.empty?).to_s, output["has_non_docker"]
        assert_equal native.size, JSON.parse(output["non_docker_matrix"])["include"].size
        docker = platforms.select { |platform| platform["docker"] }
        assert_equal docker.size, JSON.parse(output["docker_matrix"])["include"].size
        assert_equal docker.map { |platform| platform["dockerPlatform"] }.join(","), output["docker_platforms"]
        assert_equal platforms.size, JSON.parse(output["platform_matrix"])["include"].size

        File.write(File.join(dir, "release-platforms.json"), JSON.generate("platforms" => platforms))
        FileUtils.mkdir_p(File.join(dir, "dist/frontend"))
        File.write(File.join(dir, "dist/frontend/index.html"), "<html>fixture</html>")
        platforms.each do |platform|
          binary_dir = File.join(dir, "dist/binaries/codex-proxy-rs-#{platform["os"]}-#{platform["arch"]}")
          FileUtils.mkdir_p(binary_dir)
          File.write(File.join(binary_dir, platform["binary"]), "synthetic #{platform["target"]}")
        end
        _, error, status = Open3.capture3(
          { "VERSION" => "1.0.0" }, "python3", "-c",
          script("package-assets", "Package release assets", "PY"), chdir: dir
        )
        assert status.success?, error
        checksums = File.readlines(File.join(dir, "dist/release/checksums.txt"))
        assert_equal platforms.size + 2, checksums.size
        checksums.each do |line|
          checksum, name = line.split
          archive = File.join(dir, "dist/release", name)
          assert_equal Digest::SHA256.file(archive).hexdigest, checksum
          next unless name.end_with?(".tar.gz")
          listing, error, status = Open3.capture3("tar", "-tzf", archive)
          assert status.success?, error
          assert_includes listing, "codex-proxy-rs/codex-proxy-rs"
          assert_includes listing, "codex-proxy-rs/web/dist/index.html"
          assert_includes listing, "codex-proxy-rs/deploy/config.example.yaml"
        end
        assert_equal File.read(File.join(dir, "deploy/compose.yaml"))
                         .sub("${CPR_IMAGE:-ghcr.io/heykode/newcpr:latest}", "${CPR_IMAGE:-ghcr.io/heykode/newcpr:1.0.0}"),
                     File.read(File.join(dir, "dist/release/compose.yaml"))
        assert_equal File.read(File.join(dir, "deploy/config.example.yaml")),
                     File.read(File.join(dir, "dist/release/config.example.yaml"))
        FileUtils.rm(File.join(dir, "dist/binaries/codex-proxy-rs-linux-amd64/codex-proxy-rs"))
        _, error, status = Open3.capture3(
          { "VERSION" => "1.0.0" }, "python3", "-c",
          script("package-assets", "Package release assets", "PY"), chdir: dir
        )
        refute status.success?
        assert_includes error, "Missing binary artifact"
      end
    end
  end

  def test_invalid_platform_lists_fail_before_building
    [[], RESTORED.last(1), DEFAULTS * 2].each do |platforms|
      fixture(platforms) do |dir|
        _, error, status = metadata(dir)
        refute status.success?
        refute_empty error
      end
    end
  end

  def test_missing_or_mismatched_release_notes_fail_metadata
    fixture(DEFAULTS) do |dir|
      ["# v0.9.0\n\nNotes\n", "# v1.0.0\n\n"].each do |notes|
        File.write(File.join(dir, "release/notes.md"), notes)
        _, error, status = metadata(dir)
        refute status.success?
        assert_includes error, "release/notes.md"
      end
    end
  end

  # Evaluate only the small boolean grammar used by the actual workflow gates.
  def allowed?(job, states, has_native, cancelled)
    expression = JOBS.fetch(job).fetch("if").delete_prefix("${{").strip.delete_suffix("}}").strip
    expression = expression.gsub("cancelled()", cancelled.to_s)
    expression = expression.gsub("needs.release-info.outputs.has_non_docker", has_native.to_s.inspect)
    expression = expression.gsub(/needs\.([\w-]+)\.result/) { states.fetch(Regexp.last_match(1)).inspect }
    tokens = expression.scan(/"[^"]*"|'[^']*'|true|false|&&|\|\||==|!|\(|\)|\s+/).join
    assert_equal expression, tokens, "Unexpected condition syntax"
    eval(expression) # Only checked boolean literals/operators, never arbitrary workflow code.
  end

  def test_packaging_gates_optional_skip_failures_and_cancellation
    states = { "release-info" => "success", "quality" => "success",
               "build-docker-binary" => "success", "build-binary" => "skipped" }
    assert allowed?("package-assets", states, false, false)
    refute allowed?("package-assets", states, true, false)
    refute allowed?("package-assets", states, nil, false)
    states["build-binary"] = "success"
    assert allowed?("package-assets", states, true, false)
    refute allowed?("package-assets", states, false, false)
    [false, true].each do |native|
      good = states.merge("build-binary" => native ? "success" : "skipped")
      refute allowed?("package-assets", good, native, true)
      good.each_key do |job|
        %w[failure cancelled].each do |result|
          refute allowed?("package-assets", good.merge(job => result), native, false)
        end
        unless job == "build-binary" && !native
          refute allowed?("package-assets", good.merge(job => "skipped"), native, false)
        end
      end
    end
  end

  def test_publication_requires_every_direct_dependency
    states = { "release-info" => "success", "package-assets" => "success", "build-docker-image" => "success" }
    assert allowed?("publish", states, false, false)
    refute allowed?("publish", states, false, true)
    states.each_key do |job|
      %w[failure cancelled skipped].each do |result|
        refute allowed?("publish", states.merge(job => result), false, false)
      end
    end
  end
end
