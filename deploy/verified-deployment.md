# 已验证镜像部署

日常部署使用同一份经过 CI 检查的镜像，不在生产机重新编译，也不临时编写部署脚本。
本流程目前覆盖 `linux/amd64`；正式多架构 Release 继续使用既有发布工作流。

## 固定流程

1. 修改代码，提交 PR。CI 按受影响范围检查，并构建、扫描、启动验证镜像。
2. CI 留存 `image.tar.gz` 和独立的 `manifest.json`。上传本身不代表允许部署，
   整个 CI 必须成功，目标提交的完整 Security Scan 也必须成功。
3. 合并到 `main`。只有整个 Git tree 与已通过检查的合并父提交完全相同，
   工作流来源可信、镜像未过期时，才复用检查和镜像，否则正常重新运行 CI。
4. 使用统一部署命令。先下载、校验、上传镜像和备份配置，旧服务持续运行。
5. 服务端取得部署锁，再检查当前实例未被其他窗口更新，仅替换应用容器。
6. 健康检查失败则恢复旧 Compose 镜像配置并启动旧镜像，明确报告回滚结果。
   回滚固定旧镜像 ID，不依赖可能已指向新版的标签；原文件另外保留备份。

不会跳过失败的 CI，也不会把“测试被跳过”当成有效证据。合并提交和测试提交 SHA
可以不同，但 Git tree 必须一致；镜像保留实际构建提交，不伪装成合并提交。
镜像清单绑定仓库、CI run/attempt、完整源码树、迁移目录、版本、架构和归档摘要。
GitHub 下载的 ZIP 摘要与清单内的镜像归档摘要都会校验。

## 私有目标配置

部署端需要 Python 3.9+、Git、已登录的 `gh`、SSH 和 SCP。服务器需要 Docker Compose、
Python 3.9+、PyYAML（版本见 `deploy/rollout-requirements.txt`），以及操作该部署的权限。
脚本不会在服务器自动安装依赖或修改全局配置。

真实配置放在仓库外，例如 `$HOME/.config/cpr/deploy-production.json`，权限必须为 0600。
下面仅为示例，不能原样用于真实部署：

```json
{
  "repository": "example/application",
  "ssh_host": "production-alias",
  "compose_file": "/srv/application/deploy/compose.yaml",
  "container": "application-app-1",
  "service": "app",
  "health_url": "http://127.0.0.1:8080/healthz",
  "health_status": 204,
  "protected_containers": ["application-postgres-1", "application-redis-1", "other-api"],
  "config_files": ["/srv/application/deploy/config.yaml"]
}
```

SSH 目标只能是预先配置好的别名。生产和测试环境使用不同私有配置，不能由脚本猜测。
健康地址应使用稳定的本机监听地址或固定容器地址，不能依赖会变动的容器 IP。
快速路径只接受当前容器记录的单一 Compose 文件，并沿用该容器的 Compose 项目名；
多文件覆盖部署需要先单独适配，不会擅自丢弃覆盖配置。
旧镜像必须具有源码 revision 和版本标签，才能核对数据库迁移目录及大版本；
没有标签的首次接入需要人工确认基线，不提供绕过参数。

## 所有窗口共用

先同步 `main` 中的部署脚本并重读 `AGENTS.md`，再执行只读计划：

```bash
python3 release/deploy.py \
  --profile "$HOME/.config/cpr/deploy-production.json" \
  --commit <完整的-main-提交-SHA>
```

用户明确授权部署后，给同一条命令加 `--apply`。脚本不提交代码、不打标签、不发布
GitHub Release，不修改账号和指纹，也不会重启配置中的受保护容器。
工作目录里其他未提交改动不会被打包进镜像；部署脚本自身如果落后或被修改，会拒绝执行。

服务端锁为 Compose 同目录下的 `.cpr-deployment.lock`。配置及结果保存在该目录的
`.deployment-backups/`，成功部署记录保存在 `.cpr-release.json`。这些都是私有运维文件，
不能提交 Git。镜像上传目录保留用于诊断，清理时应确认没有正在运行的部署。

## 何时重新检查

- 产物保留 14 天，但快速复用证据最多有效 7 天；不接受未来时间或过期证据。
- 源码树、工作流、依赖、构建参数所属文件发生变化，不能借用旧结果。
- GitHub API、权限、产物摘要或来源校验失败时，宁可停止，也不默默跳过。
- 目标 CI 正在执行时先等待；不存在合格产物时，从 `main` 手动运行 CI。
  日常部署不触发多架构 Release 流程。旧历史提交没有产物时，应单独制定回滚方案。
- 数据库迁移目录变化或大版本变化会退出快速通道。需要单独审查在线备份、
  迁移兼容性及回滚策略，不能仅凭新镜像健康就自动回滚数据库。

源码编译和上传时间不等于停机时间。切换仍使用原来的优雅退出预算，
不会为了宣称“几秒”强杀正在处理的请求。单实例 SSE/WS 连接不能跨进程迁移，
旧长连接可能需要重连；脚本测量健康接口不可用区间，而不是宣称所有请求零中断。

## 验证

```bash
python3 -m unittest discover -s release/tests -p 'test_*.py' -v
python3 -m unittest discover -s deploy/tests -p 'test_*.py' -v
```

在专用测试机可额外运行真实 Docker 演练，使用独立临时项目、回环端口，
结束后清理自己的容器和镜像，不操作现有应用：

```bash
CPR_ROLLOUT_DOCKER_TEST=1 python3 -m unittest discover \
  -s deploy/tests -p 'test_docker_rollout.py' -v
```

测试覆盖来源/摘要/有效期、相同树复用、旧窗口和配置竞态、迁移拒绝、失败回滚及
受保护服务边界。真实工作流的产物往返验证需要本次改动进入 GitHub Actions 后执行；
本地单元测试不能替代该验收。
