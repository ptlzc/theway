# grpc-path-context

Issue: #68 · Branch: `feat/issue-68-grpc-expose-path-context-and-support-dyn` · DAG: `issue-68-grpc-path-context`

gRPC 暴露 daemon 路径上下文（home/base/work_dir/skills_dirs），并新增 `SetSkillDirs`
在运行中更新 skill 扫描根 + 热重载；home/work_dir/base 保持启动固定。
