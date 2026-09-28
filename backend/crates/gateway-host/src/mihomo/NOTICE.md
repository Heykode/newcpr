# 来源与修改说明

Mihomo管理、动态代理解析、地区过滤、BPS会话池及评分参考并改写自`ranxi2001/sub2api@030c1fd776f1e4523728665c0a6127a0f4dbac33`的`backend/internal/mihomo/*.go`及相应管理、Excel BPS服务。

上游许可原文保存在`UPSTREAM-LICENSE`（GNU LGPL v3）。保留上游权利人的许可；CPR其他文件的声明不抹去此处的上游许可。

2026-09-28适配：Rust端口拆分、异步管理、CPR账号配置/CA、租约生命周期、匿名隔离、规范化节点身份、父进程管道监督、CPR界面与存储。

Mihomo内核由管理员触发下载，固定`v1.19.31`并校验官方资产SHA-256，不纳入Git；内核版权和许可归其上游项目所有。
