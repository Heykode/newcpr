require 'minitest/autorun'
require 'minitest/mock'
require 'yaml'
require_relative '../publish-release'
require_relative '../published-channels'

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
    check_index = steps.index { |step| step['run'].to_s.include?('published-channels.rb check') }
    publish_index = steps.index { |step| step['run'].to_s.include?('published-channels.rb publish') }
    assert_operator check_index, :<, upload
    assert_operator upload, :<, publish
    assert_operator publish, :<, publish_index
    recovery = workflow.fetch('jobs').fetch('recover-index')
    assert_equal job.fetch('concurrency'), recovery.fetch('concurrency')
    assert_equal 'inputs.index_only', recovery.fetch('if')
    assert recovery.fetch('steps').any? { |step| step['run'].to_s.include?('published-channels.rb publish') }
    assert_equal true, steps[upload].fetch('with').fetch('draft')
    assert_equal 'false', steps[upload].fetch('with').fetch('make_latest')
    assert steps[0...upload].any? { |step| step['run'].to_s.include?('cosign sign') }
    %w[build-docker-binary build-binary].each do |name|
      build_steps = workflow.fetch('jobs').fetch(name).fetch('steps')
      build = build_steps.find { |step| step['name'] == 'Build binary' }
      assert_equal './.github/actions/build-binary', build.fetch('uses')
      assert_equal '${{ needs.release-info.outputs.build_type }}', build.fetch('env').fetch('CPR_BUILD_TYPE')
    end
  end
  def test_published_channels_promote_only_allowed_stages_and_experimental_baselines
    document = { 'published' => {} }
    assert PublishedChannels.advance(document, '3.42.0-beta.1')
    assert_equal({ 'beta' => '3.42.0-beta.1', 'alpha' => '3.42.0-beta.1' }, document['published']['3'])
    PublishedChannels.advance(document, '3.42.0')
    assert_equal %w[stable rc beta alpha].to_h { |stage| [stage, '3.42.0'] }, document['published']['3']
    refute PublishedChannels.advance(document, '3.41.0')
    PublishedChannels.advance(document, '3.43.0-exp.1')
    assert_equal '3.43.0-exp.1', document.dig('published', '3', 'exp', '3.43.0')
    assert_equal '3.42.0', document.dig('published', '3', 'stable')
    PublishedChannels.advance(document, '4.0.0')
    assert_equal '3.42.0', document.dig('published', '3', 'stable')
  end

  def test_channel_publication_checks_assets_and_uses_the_observed_main_sha
    publisher = PublishedChannels::Publisher.new('example/repo')
    events = []
    main = { 'version' => '3.43.0-exp.1', 'published' => { '4' => { 'stable' => '4.0.0' } } }
    publisher.stub(:verify_assets, ->(version) { events << [:verified, version] }) do
      publisher.stub(:api, lambda { |endpoint, **options|
        events << [endpoint, options]
        if options[:method] == 'PUT'
          data = options.fetch(:data)
          assert_equal 'main', data.fetch(:branch)
          assert_equal 'observed-file-sha', data.fetch(:sha)
          written = YAML.safe_load(Base64.decode64(data.fetch(:content)))
          assert_equal main['version'], written['version']
          assert_equal '4.0.0', written.dig('published', '4', 'stable')
          assert_equal '3.42.1', written.dig('published', '3', 'stable')
          {}
        else
          { 'sha' => 'observed-file-sha', 'content' => Base64.strict_encode64(YAML.dump(main)) }
        end
      }) { publisher.publish('3.42.1') }
    end
    assert_equal [:verified, '3.42.1'], events.first
    assert_equal 'repos/example/repo/contents/release/version.yaml?ref=main', events[1][0]
    assert_equal :PUT.to_s, events[2][1][:method]
  end

  def test_missing_assets_or_a_concurrent_main_edit_cannot_advance_channels
    publisher = PublishedChannels::Publisher.new('example/repo')
    publisher.stub(:verify_assets, ->(_) { raise 'incomplete assets' }) do
      publisher.stub(:api, ->(*) { flunk 'must not read or write main before asset verification' }) do
        assert_raises(RuntimeError) { publisher.publish('3.42.1') }
      end
    end
    publisher.stub(:verify_assets, ->(_) {}) do
      publisher.stub(:api, lambda { |_, **options|
        raise 'HTTP 409: file changed' if options[:method] == 'PUT'
        { 'sha' => 'stale-sha', 'content' => Base64.strict_encode64(YAML.dump({ 'version' => '3.42.0', 'published' => {} })) }
      }) do
        error = assert_raises(RuntimeError) { publisher.publish('3.42.1') }
        assert_includes error.message, '409'
      end
    end
  end

  def checked_publication(outcome: 'success', concurrent_main: false)
    publisher = PublishedChannels::Publisher.new('example/repo')
    events = []
    document = { 'version' => '3.43.0', 'published' => {} }
    api = lambda do |endpoint, **options|
      events << [endpoint, options]
      method = options.fetch(:method, 'GET')
      case
      when endpoint.end_with?('/git/ref/heads/main')
        { 'object' => { 'sha' => 'observed-main' } }
      when endpoint.end_with?('?ref=observed-main')
        { 'sha' => 'observed-file', 'content' => Base64.strict_encode64(YAML.dump(document)) }
      when method == 'PUT'
        data = options.fetch(:data)
        assert_match %r{\Acodex/published-v3\.42\.1-}, data.fetch(:branch)
        assert_equal 'observed-file', data.fetch(:sha)
        written = YAML.safe_load(Base64.decode64(data.fetch(:content)))
        assert_equal '3.43.0', written.fetch('version')
        assert_equal '3.42.1', written.dig('published', '3', 'stable')
        { 'commit' => { 'sha' => 'candidate' } }
      when endpoint.include?('/runs?')
        { 'workflow_runs' => [
          { 'head_sha' => 'unrelated', 'status' => 'completed', 'conclusion' => 'success' },
          { 'head_sha' => 'candidate', 'status' => 'completed', 'conclusion' => outcome }
        ] }
      when method == 'PATCH'
        assert_equal({ sha: 'candidate', force: false }, options.fetch(:data))
        raise 'main moved; fast-forward rejected' if concurrent_main
        {}
      else
        nil # dispatch and DELETE return HTTP 204 without a JSON body
      end
    end
    publisher.stub(:verify_assets, ->(version) { events << [:assets, version] }) do
      publisher.stub(:api, api) do
        if outcome != 'success' || concurrent_main
          assert_raises(RuntimeError) { publisher.publish('3.42.1', checked_main: true) }
        else
          publisher.publish('3.42.1', checked_main: true)
        end
      end
    end
    events
  end

  def test_protected_main_checks_the_exact_candidate_before_fast_forward
    events = checked_publication
    dispatch = events.index { |endpoint, _| endpoint.to_s.end_with?('/dispatches') }
    promote = events.index { |_, options| options.is_a?(Hash) && options[:method] == 'PATCH' }
    assert_operator dispatch, :<, promote
    assert_equal({ privacy_only: true }, events[dispatch][1].fetch(:data).fetch(:inputs))
    assert_equal 'DELETE', events.last[1].fetch(:method)
    %w[publish recover-index].each do |name|
      job = YAML.load_file(File.expand_path('../../.github/workflows/release.yml', __dir__)).fetch('jobs').fetch(name)
      assert_equal 'write', job.fetch('permissions').fetch('actions')
      step = job.fetch('steps').find { |item| item['run'].to_s.include?('published-channels.rb publish') }
      assert_equal 'true', step.fetch('env').fetch('PUBLISHED_CHANNELS_CHECKED_MAIN')
    end
  end

  def test_failed_privacy_or_concurrent_main_preserves_the_candidate_without_force
    events = checked_publication(outcome: 'failure')
    refute events.any? { |_, options| options.is_a?(Hash) && %w[PATCH DELETE].include?(options[:method]) }
    events = checked_publication(concurrent_main: true)
    refute events.any? { |_, options| options.is_a?(Hash) && options[:method] == 'DELETE' }
  end

  def test_github_empty_success_response_is_accepted
    publisher = PublishedChannels::Publisher.new('example/repo')
    Open3.stub(:capture3, response('', '', true)) do
      assert_nil publisher.api('repos/example/repo/actions/workflows/security-scan.yml/dispatches', method: 'POST', data: {})
    end
  end

end
