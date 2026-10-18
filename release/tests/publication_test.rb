require 'minitest/autorun'
require 'minitest/mock'
require 'yaml'
require_relative '../publish-release'

class PublicationTest < Minitest::Test
  DIGEST = "sha256:#{'a' * 64}"

  def test_latest_never_moves_back_or_to_prerelease
    assert ReleasePublication.can_promote?('3.30.0', ['v3.29.1', '3.29.0'])
    assert ReleasePublication.can_promote?('3.30.0', [nil, nil])
    assert ReleasePublication.can_promote?('3.30.0', ['v3.30.0', '3.30.0'])
    refute ReleasePublication.can_promote?('3.29.1', ['v3.30.0', '3.29.0'])
    refute ReleasePublication.can_promote?('3.29.1', ['v3.29.0', '3.30.0'])
    refute ReleasePublication.can_promote?('3.30.0-rc.1', [nil, nil])
    refute ReleasePublication.can_promote?('3.30.0+other', ['3.30.0+first'])
    assert_raises(RuntimeError) { ReleasePublication.can_promote?('3.30.0', ['unknown']) }
  end

  def publish(version: '3.30.0', latest: '3.29.1', fail_latest: false)
    events = []
    tag = lambda do |image, digest, name|
      assert_equal 'registry.example/repo', image
      assert_equal DIGEST, digest
      events << [:tag, name]
      raise 'synthetic promotion failure' if fail_latest && name == 'latest'
    end
    run = ->(*args) { events << [:api, *args]; '' }
    ReleasePublication.stub(:tag, tag) do
      ReleasePublication.stub(:run, run) do
        ReleasePublication.stub(:latest_release, latest) do
          ReleasePublication.stub(:latest_image, latest) do
            begin
              ReleasePublication.publish(version: version, image: 'registry.example/repo', digest: DIGEST,
                                         repository: 'example/repo', release_id: '123')
            rescue RuntimeError => error
              raise unless fail_latest && error.message == 'synthetic promotion failure'
            end
          end
        end
      end
    end
    events
  end

  def test_publication_order_and_retry
    events = publish
    assert_equal [:tag, '3.30.0'], events[0]
    assert_includes events[1], 'draft=false'
    assert_includes events[1], 'make_latest=false'
    assert_equal [:tag, 'latest'], events[2]
    assert_includes events[3], 'make_latest=true'
    assert_equal events, publish(latest: '3.30.0'), 'retry can repair a partial latest promotion'
  end

  def test_older_and_prereleases_publish_without_latest
    [publish(latest: '3.31.0'), publish(version: '3.30.0-rc.1')].each do |events|
      assert_equal 2, events.size
      refute events.any? { |event| event.include?('make_latest=true') }
    end
  end

  def test_failed_image_promotion_does_not_promote_release
    events = publish(fail_latest: true)
    assert_equal [:tag, 'latest'], events.last
    refute events.any? { |event| event.include?('make_latest=true') }
  end

  def test_digest_is_verified_before_publishing
    commands = []
    ReleasePublication.stub(:run, ->(*args) { commands << args; 'sha256:wrong' }) do
      assert_raises(RuntimeError) { ReleasePublication.tag('registry.example/repo', DIGEST, '3.30.0') }
    end
    assert_equal 'create', commands[0][3]
    assert_equal 'inspect', commands[1][3]
  end

  def response(output, error, successful)
    status = Object.new
    status.define_singleton_method(:success?) { successful }
    [output, error, status]
  end

  def test_latest_release_distinguishes_first_publish_from_network_or_auth_failure
    Open3.stub(:capture3, response("HTTP/2.0 404 Not Found\n\n{}", 'not found', false)) do
      assert_nil ReleasePublication.latest_release('example/repo')
    end
    Open3.stub(:capture3, response("HTTP/2.0 200 OK\r\n\r\n{\"tag_name\":\"v3.30.0\"}", '', true)) do
      assert_equal 'v3.30.0', ReleasePublication.latest_release('example/repo')
    end
    Open3.stub(:capture3, response('', 'authentication failed', false)) do
      assert_raises(RuntimeError) { ReleasePublication.latest_release('example/repo') }
    end
  end

  def test_latest_image_requires_consistent_platform_versions_and_fails_closed
    image = 'registry.example/repo'
    Open3.stub(:capture3, response('', "#{image}:latest: not found", false)) do
      assert_nil ReleasePublication.latest_image(image)
    end
    Open3.stub(:capture3, response('', 'authentication failed', false)) do
      assert_raises(RuntimeError) { ReleasePublication.latest_image(image) }
    end
    configs = ->(versions) do
      JSON.generate(versions.each_with_index.to_h do |version, index|
        ["platform-#{index}", { 'config' => { 'Labels' => { 'org.opencontainers.image.version' => version } } }]
      end)
    end
    Open3.stub(:capture3, response(configs.call(['3.30.0', '3.30.0']), '', true)) do
      assert_equal '3.30.0', ReleasePublication.latest_image(image)
    end
    Open3.stub(:capture3, response(configs.call(['3.30.0', '3.29.1']), '', true)) do
      assert_raises(RuntimeError) { ReleasePublication.latest_image(image) }
    end
  end

  def test_workflow_prepares_assets_and_signatures_before_publication
    workflow = YAML.load_file(File.expand_path('../../.github/workflows/release.yml', __dir__))
    job = workflow.fetch('jobs').fetch('publish')
    assert_equal 'release-publication', job.fetch('concurrency').fetch('group')
    assert_equal 'max', job.fetch('concurrency').fetch('queue')
    refute job.fetch('concurrency').fetch('cancel-in-progress', false)
    steps = job.fetch('steps')
    upload = steps.index { |step| step['id'] == 'release' }
    publish = steps.index { |step| step['run'].to_s.include?('ruby release/publish-release.rb') }
    assert_operator upload, :<, publish
    assert_equal true, steps[upload].fetch('with').fetch('draft')
    assert_equal 'false', steps[upload].fetch('with').fetch('make_latest')
    assert steps[0...upload].any? { |step| step['run'].to_s.include?('cosign sign') }
    %w[build-docker-binary build-binary].each do |name|
      build_steps = workflow.fetch('jobs').fetch(name).fetch('steps')
      cache = build_steps.find { |step| step['name'] == 'Cache Cargo' }.fetch('with')
      refute_includes cache.fetch('key'), 'outputs.git_sha'
      assert_includes cache.fetch('key'), 'rust-toolchain.toml'
      build = build_steps.find { |step| step['name'] == 'Build binary' }
      assert_equal '${{ needs.release-info.outputs.build_type }}', build.fetch('env').fetch('CPR_BUILD_TYPE')
    end
  end
end
