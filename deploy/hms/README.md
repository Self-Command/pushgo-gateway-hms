# HMS 网关通用部署

每次推送 main 后自动构建 linux/amd64 和 linux/arm64 镜像；两个架构均通过启动检查后才更新 latest 和发布 GitHub Release。`pushgo-gateway-hms-deploy.tar.gz` 可直接下载，包含配置模板、Compose、固定源码镜像引用和说明，不包含真实密钥。用户于 2026-10-06 授权这个自动发布流程。

1. 解压包，复制 `.env.example` 为 `.env`。填写独立 `PUSHGO_TOKEN`、华为 `PUSHGO_HUAWEI_APP_ID` 和网关的 `PUSHGO_PUBLIC_BASE_URL`。
2. 创建 `secrets/huawei-app-secret.txt`，填入 AGC 应用 OAuth 的 Client Secret。目录权限设置为 0700，文件 0644，让只读单文件挂载可供容器读取。密钥不得上传仓库。
3. `docker compose pull && docker compose up -d`。默认绑定 127.0.0.1:6666，由你现有的 HTTPS 反向代理转发。局域网测试可将 `PUSHGO_BIND_ADDRESS` 设置为电脑的当前局域网 IP，同时设置正确的 HTTP 公共地址和防火墙规则。
4. 在 HMS APK 设置中填写这个网关地址和 `PUSHGO_TOKEN`，再创建/订阅频道。打卡后端是另一个独立服务，单独部署。

数据保存在专用 gateway-data 卷。更新前备份数据库；`docker compose pull && docker compose up -d` 更新镜像，停止容器不会删除卷。镜像的 `sha-<完整提交>` 引用可固定版本，`latest` 会随成功构建更新。

## 完整任务卡片大小

`PUSHGO_METADATA_MAX_VALUE_BYTES` 控制每个 metadata 标量的 UTF-8 字节上限，默认仍为 512，可设置 1–16384。需要较长任务卡片时，可在 `.env` 中设为 `8192` 并重建网关容器。无效值会阻止启动。HTTP 请求总上限仍为 32 KiB，字段名、标量格式及供应商载荷限制保持不变。超出供应商直投载荷的消息沿用已有唤醒拉取机制，不截断任务内容。网关客户端无需重新安装。
