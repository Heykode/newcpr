require 'fileutils'
require 'json'
require 'open3'
require 'tmpdir'

# 使用临时 Git 仓库验证远端状态和只读边界，不访问真实 GitHub 或修改当前仓库
ROOT = File.expand_path('../..', __dir__)

def assert(condition, message)
  raise message unless condition
end

def git(*arguments)
  output, error, status = Open3.capture3('git', *arguments)
  raise "git #{arguments.first}: #{error}" unless status.success?
  output.strip
end

def commit(message)
  git('add', '.')
  git('commit', '-qm', message)
end

Dir.mktmpdir('release-checks-') do |directory|
  remote = File.join(directory, 'remote.git')
  system('git', 'init', '--bare', '-q', remote, exception: true)
  checkout = File.join(directory, 'checkout')
  FileUtils.mkdir_p(checkout)
  Dir.chdir(checkout) do
    git('init', '-q', '-b', 'main')
    git('config', 'user.email', 'test@example.invalid')
    git('config', 'user.name', 'Release test')
    files = %w[
      release/publish release/version.yaml release/notes.md release/platforms.yaml
      release/toolchains.rb release/release-metadata.rb
      backend/rust-toolchain.toml deploy/Dockerfile
    ]
    files.each do |path|
      FileUtils.mkdir_p(File.dirname(path))
      FileUtils.cp(File.join(ROOT, path), path)
    end
    commit('initial source')
    git('remote', 'add', 'origin', remote)
    git('push', '-qu', 'origin', 'main')
    File.write('release/notes.md', "# v99.0.0\n\nFixture notes\n")

    bin = File.join(directory, 'bin')
    FileUtils.mkdir_p(bin)
    File.write(File.join(bin, 'gh'), <<~RUBY)
      #!/usr/bin/env ruby
      case ARGV
      when ['auth', 'status'] then exit 0
      when ['repo', 'view', #{remote.inspect}, '--json', 'nameWithOwner', '--jq', '.nameWithOwner']
        puts 'owner/repo'
      else
        abort "unexpected gh mutation: \#{ARGV.inspect}"
      end
    RUBY
    File.chmod(0o755, File.join(bin, 'gh'))
    environment = { 'PATH' => "#{bin}:#{ENV.fetch('PATH')}" }
    snapshot = lambda do
      [git('show-ref'), git('status', '--porcelain'), File.binread('.git/index'),
       files.to_h { |path| [path, File.binread(path)] },
       git('ls-remote', 'origin'), File.exist?('.git/FETCH_HEAD')]
    end
    check = lambda do |success, message = nil|
      before = snapshot.call
      output, error, status = Open3.capture3(environment, 'bash', 'release/publish', '--check', '99.0.0')
      assert(status.success? == success, "preflight outcome: #{output}#{error}")
      assert("#{output}#{error}".include?(message), "missing error: #{message}; #{error}") if message
      assert(snapshot.call == before, 'preflight modified files, refs, index, or FETCH_HEAD')
    end
    check.call(true, 'Preflight passed')
    File.write('deploy/Dockerfile', File.read('deploy/Dockerfile') + "\n# fixture\n")
    check.call(false, 'unstaged changes')
    git('checkout', '--', 'deploy/Dockerfile')
    git('tag', 'v99.0.0')
    check.call(false, 'local tag already exists')
    git('push', '-q', 'origin', 'refs/tags/v99.0.0')
    git('tag', '-d', 'v99.0.0')
    check.call(false, 'remote tag already exists')
    git('push', '-q', 'origin', ':refs/tags/v99.0.0')

    # 远端前进但没有本地对象时，提示同步而不隐式 fetch
    other = File.join(directory, 'other')
    git('clone', '-q', '-b', 'main', remote, other)
    Dir.chdir(other) do
      git('config', 'user.email', 'test@example.invalid')
      git('config', 'user.name', 'Release test')
      File.write('remote-only', 'change')
      commit('remote advance')
      git('push', '-q')
    end
    check.call(false, 'unseen commits')
    git('fetch', '-q', 'origin')
    check.call(false, 'behind')
    FileUtils.mv(remote, "#{remote}.hidden")
    # 读取远端失败不能当作 tag 不存在，仍须保持本地不变
    before = [git('show-ref'), git('status', '--porcelain'), File.binread('.git/index')]
    _, error, status = Open3.capture3(environment, 'bash', 'release/publish', '--check', '99.0.0')
    assert(!status.success? && error.include?('failed to read remote refs'), 'remote failure was ignored')
    assert(before == [git('show-ref'), git('status', '--porcelain'), File.binread('.git/index')], 'remote failure changed local state')
    FileUtils.mv("#{remote}.hidden", remote)

    File.write('release/version.yaml', "version: \"99.0.0\"\n")
    commit('release metadata')
    # 在同一个隔离仓库走实际提交与原子推送路径，dispatch 仍由本地替身接收
    git('merge', '--no-edit', 'origin/main')
    File.write('release/notes.md', "# v100.0.0\n\nPublish fixture notes\n")
    dispatch = File.join(directory, 'dispatch.json')
    mock = File.join(bin, 'gh')
    File.write(mock, File.read(mock).sub("else\n", <<~RUBY))
      when ['workflow', 'run', 'release.yml', '--repo', 'owner/repo', '--ref', 'main', '--raw-field', 'tag=v100.0.0']
        File.write(#{dispatch.inspect}, ARGV.join(' '))
      else
    RUBY
    output, error, status = Open3.capture3(environment, 'bash', 'release/publish', '100.0.0')
    assert(status.success?, "publish failed: #{output}#{error}")
    assert(File.exist?(dispatch), 'workflow was not dispatched')
    assert(git('log', '-1', '--format=%s') == 'chore(release): prepare v100.0.0', 'wrong release commit')
    assert(git('status', '--porcelain').empty?, 'release left tracked changes')
    assert(git('rev-parse', 'HEAD') == git('rev-parse', 'v100.0.0^{commit}'), 'tag differs from release commit')
    assert(git('ls-remote', 'origin', 'refs/tags/v100.0.0').split.first == git('rev-parse', 'v100.0.0'), 'tag not pushed')
  end
end
puts 'Release preflight and CI evidence checks passed'
