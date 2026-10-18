require 'minitest/autorun'
require 'tmpdir'
require 'fileutils'
require_relative '../toolchains'

class ToolchainsTest < Minitest::Test
  def fixture
    Dir.mktmpdir do |root|
      FileUtils.mkdir_p(["#{root}/backend", "#{root}/deploy"])
      File.write("#{root}/backend/rust-toolchain.toml", "[toolchain]\nchannel = \"1.99.1\"\n")
      File.write("#{root}/deploy/Dockerfile", <<~DOCKER)
        FROM node:26.12.0-bookworm-slim@sha256:#{'1' * 64} AS frontend-builder
        FROM rust:1.99.1-bookworm@sha256:#{'2' * 64} AS backend-toolchain
        FROM backend-toolchain AS backend-builder
      DOCKER
      yield root
    end
  end

  def test_reads_cached_builder_stage_and_node_version
    fixture do |root|
      values = BuildToolchains.read(root)
      assert_equal '1.99.1', values['rust']
      assert_equal '26.12.0', values['node']
      assert_equal "rust:1.99.1-bookworm@sha256:#{'2' * 64}", values['rust_image']
    end
  end

  def test_rejects_drift_and_unpinned_images
    fixture do |root|
      path = "#{root}/deploy/Dockerfile"
      original = File.read(path)
      [original.sub('rust:1.99.1', 'rust:1.98.0'),
       original.sub("@sha256:#{'1' * 64}", '')].each do |invalid|
        File.write(path, invalid)
        assert_raises(RuntimeError) { BuildToolchains.read(root) }
      end
    end
  end

  def test_repository_declarations_match
    values = BuildToolchains.read(File.expand_path('../..', __dir__))
    assert_equal '1.99.1', values['rust']
    assert_equal '26.12.0', values['node']
  end
end
