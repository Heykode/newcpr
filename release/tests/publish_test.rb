require "minitest/autorun"
require "tmpdir"
require "fileutils"
require "open3"

class PublishTest < Minitest::Test
  SCRIPT = File.expand_path("../publish", __dir__)

  def setup
    @tmp = Dir.mktmpdir("cpr-publish-test")
    @repo = File.join(@tmp, "repo")
    @remote = File.join(@tmp, "remote.git")
    @bin = File.join(@tmp, "bin")
    FileUtils.mkdir_p([@repo, @bin, File.join(@repo, "release")])
    @env = {
      "GIT_CONFIG_GLOBAL" => File::NULL, "GIT_CONFIG_NOSYSTEM" => "1",
      "GIT_AUTHOR_NAME" => "Synthetic Test", "GIT_COMMITTER_NAME" => "Synthetic Test",
      "GIT_AUTHOR_EMAIL" => "test@users.noreply.github.com",
      "GIT_COMMITTER_EMAIL" => "test@users.noreply.github.com",
      "PATH" => "#{@bin}:#{ENV.fetch("PATH")}"
    }
    # Only gh is mocked. Git fetch/tag/push run against a disposable local bare repo.
    File.write(File.join(@bin, "gh"), <<~SH)
      #!/bin/sh
      case "$1" in
        auth) exit 0 ;;
        repo) echo example/test ;;
        workflow) exit 0 ;;
        *) exit 1 ;;
      esac
    SH
    FileUtils.chmod(0755, File.join(@bin, "gh"))
    command("git", "init", "--bare", "--quiet", @remote)
    git("init", "--quiet", "-b", "main")
    git("config", "user.name", "Synthetic Test")
    git("config", "user.email", "test@users.noreply.github.com")
    File.write(File.join(@repo, "release/version.yaml"), "version: \"0.1.0\"\n")
    File.write(File.join(@repo, "release/notes.md"), "# v0.1.0\n\nInitial\n")
    File.write(File.join(@repo, "source.txt"), "unchanged\n")
    git("add", ".")
    git("commit", "--quiet", "-m", "initial")
    git("remote", "add", "origin", @remote)
    git("push", "--quiet", "-u", "origin", "main")
  end

  def teardown
    FileUtils.remove_entry(@tmp)
  end

  def test_unstaged_and_staged_notes_are_published_with_version
    [false, true].each do |staged|
      version = staged ? "0.1.2" : "0.1.1"
      notes = "# v#{version}\n\nSynthetic update\n"
      File.write(File.join(@repo, "release/notes.md"), notes)
      git("add", "release/notes.md") if staged
      output, status = publish(version)
      assert status.success?, output
      assert_equal notes, git("show", "v#{version}:release/notes.md")
      assert_equal "version: \"#{version}\"\n", git("show", "v#{version}:release/version.yaml")
      assert_equal "", git("status", "--porcelain")
      assert_equal git("rev-parse", "HEAD"), git("rev-parse", "origin/main")
    end
  end

  def test_rejects_clean_wrong_or_empty_notes_without_mutating_history
    initial = git("rev-parse", "HEAD")
    [nil, "# v9.9.9\n\nWrong\n", "# v0.1.1\n\n"].each do |notes|
      File.write(File.join(@repo, "release/notes.md"), notes) if notes
      output, status = publish("0.1.1")
      refute status.success?, output
      assert_equal initial, git("rev-parse", "HEAD")
      assert_equal "", git("tag", "--list")
      assert_equal "version: \"0.1.0\"\n", File.read(File.join(@repo, "release/version.yaml"))
    end
  end

  def test_rejects_unrelated_staged_and_unstaged_changes
    File.write(File.join(@repo, "release/notes.md"), "# v0.1.1\n\nSynthetic update\n")
    File.write(File.join(@repo, "source.txt"), "pending implementation\n")
    [false, true].each do |staged|
      git("add", "source.txt") if staged
      output, status = publish("0.1.1")
      refute status.success?, output
      assert_equal "", git("tag", "--list")
      assert_equal "version: \"0.1.0\"\n", File.read(File.join(@repo, "release/version.yaml"))
    end
  end

  def publish(version)
    Open3.capture2e(@env, "bash", SCRIPT, version, chdir: @repo)
  end

  def git(*args)
    command("git", *args)
  end

  def command(*args)
    output, status = Open3.capture2e(@env, *args, chdir: @repo)
    assert status.success?, output
    output
  end
end
