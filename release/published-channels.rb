require 'base64'
require 'uri'
require 'securerandom'
require_relative 'release-metadata'

# 各发行线共用 main 通道表；只合并本次已核实的版本
module PublishedChannels
  PATH = 'release/version.yaml'
  LIMIT = 128 * 1024

  def self.advance(document, version)
    version = ReleaseMetadata.version(version)
    major, = version.split('.', 2)
    core, stage = version.split('+', 2).first.split('-', 2)
    channels = (document.fetch('published')[major] ||= {})
    if stage&.start_with?('exp.')
      channels = (channels['exp'] ||= {})
      keys = [core]
    else
      stages = %w[stable rc beta alpha]
      keys = stages.drop(stages.index(stage ? stage.split('.').first : 'stable'))
    end
    incoming = ReleaseMetadata.sort_key(version)
    changed = false
    keys.each do |key|
      current = channels[key]
      if current
        comparison = incoming <=> ReleaseMetadata.sort_key(current)
        raise "Conflicting release identities: #{current}, #{version}" if comparison.zero? && current != version
        next unless comparison.positive?
      end
      channels[key] = version
      changed = true
    end
    changed
  end

  class Publisher
    def api(endpoint, method: 'GET', data: nil, optional: false)
      args = ['gh', 'api', endpoint, '--method', method]
      args.concat(%w[--input -]) if data
      out, error, status = Open3.capture3(*args, stdin_data: data ? JSON.generate(data) : '')
      return nil if optional && !status.success? && error.include?('(HTTP 404)')
      raise "Cannot access #{endpoint}: #{error.strip}" unless status.success?
      out.strip.empty? ? nil : JSON.parse(out)
    end

    def initialize(repository)
      @base = "repos/#{repository}"
    end

    def find_release(version)
      api("#{@base}/releases/tags/v#{ReleaseMetadata.version(version)}", optional: true)
    end

    def verify_assets(version)
      release = find_release(version)
      tag = "v#{version}"
      raise "Release is not public or tag differs: #{tag}" unless release && release.fetch('tag_name') == tag &&
        !release.fetch('draft') && release.fetch('prerelease') == ReleaseMetadata.prerelease?(version)
      data = api("#{@base}/contents/release/platforms.yaml?ref=#{URI.encode_www_form_component(tag)}")
      platforms = YAML.safe_load(Base64.decode64(data.fetch('content'))).fetch('platforms')
      raise "Missing platforms for #{tag}" if platforms.empty?
      expected = platforms.map { |p| ReleaseMetadata.archive_name(version, p) }
      expected.concat(%w[checksums.txt compose.yaml config.example.yaml])
      uploaded = release.fetch('assets').filter_map do |asset|
        asset.fetch('name') if asset.fetch('state') == 'uploaded' && asset.fetch('size').positive?
      end
      missing = expected - uploaded
      raise "Incomplete release #{tag}: #{missing.join(', ')}" unless missing.empty?
    end

    def check_unpublished(version)
      existing = find_release(version)
      raise "Release v#{version} already public; use index-only recovery" if existing && !existing.fetch('draft')
    end

    def publish(version, checked_main: false)
      verify_assets(version)
      # 读取 main 的当前文件，实验分支和较晚结束的旧构建都不能覆盖其他通道或构建版本
      endpoint = "#{@base}/contents/#{PATH}"
      base = checked_main ? api("#{@base}/git/ref/heads/main").fetch('object').fetch('sha') : 'main'
      file = api("#{endpoint}?ref=#{base}")
      document = YAML.safe_load(Base64.decode64(file.fetch('content')))
      return unless PublishedChannels.advance(document, version)

      content = YAML.dump(document).delete_prefix("---\n")
      raise 'Published index exceeds the client size limit' if content.bytesize > LIMIT
      return publish_checked_main(version, base, file, content) if checked_main

      # 按读取的文件 SHA 写入；并发备版改过同一文件时交给恢复任务重新合并
      api(endpoint, method: 'PUT', data: {
        branch: 'main', sha: file.fetch('sha'), content: Base64.strict_encode64(content),
        message: "chore(release): publish v#{version}"
      })
    end

    # NewCPR main requires Privacy Guard even for release metadata. Stage the
    # official index result, run that existing check on its exact commit, then
    # fast-forward without bypassing branch protection or concurrent main edits.
    def publish_checked_main(version, base, file, content)
      branch = "codex/published-v#{version}-#{SecureRandom.hex(6)}"
      api("#{@base}/git/refs", method: 'POST', data: { ref: "refs/heads/#{branch}", sha: base })
      candidate = api("#{@base}/contents/#{PATH}", method: 'PUT', data: {
        branch: branch, sha: file.fetch('sha'), content: Base64.strict_encode64(content),
        message: "chore(release): publish v#{version}"
      }).fetch('commit').fetch('sha')
      # GITHUB_TOKEN pushes do not start workflows; an explicit dispatch does.
      workflow = "#{@base}/actions/workflows/security-scan.yml"
      api("#{workflow}/dispatches", method: 'POST', data: { ref: branch, inputs: { privacy_only: true } })
      wait_for_privacy(workflow, branch, candidate)
      api("#{@base}/git/refs/heads/main", method: 'PATCH', data: { sha: candidate, force: false })
      api("#{@base}/git/refs/heads/#{branch}", method: 'DELETE')
    end

    def wait_for_privacy(workflow, branch, candidate)
      deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + 900
      loop do
        runs = api("#{workflow}/runs?branch=#{URI.encode_www_form_component(branch)}&event=workflow_dispatch&per_page=20")
        run = runs.fetch('workflow_runs').find { |item| item.fetch('head_sha') == candidate }
        if run && run.fetch('status') == 'completed'
          raise "Privacy Guard failed for #{branch}; main was not changed" unless run.fetch('conclusion') == 'success'
          return
        end
        raise "Privacy Guard timed out for #{branch}; main was not changed" if Process.clock_gettime(Process::CLOCK_MONOTONIC) >= deadline
        sleep 5
      end
    end
  end
end

if $PROGRAM_NAME == __FILE__
  mode, raw = ARGV
  version = ReleaseMetadata.version(raw || '')
  publisher = PublishedChannels::Publisher.new(ENV.fetch('GITHUB_REPOSITORY'))
  case mode
  when 'check' then publisher.check_unpublished(version)
  when 'publish' then publisher.publish(version, checked_main: ENV['PUBLISHED_CHANNELS_CHECKED_MAIN'] == 'true')
  else abort 'usage: published-channels.rb check|publish VERSION'
  end
end
