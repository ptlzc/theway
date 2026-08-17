# daemon-path-context

Issue: #66 · Branch: `feat/issue-66-daemon-path-context-controller-supplied-` · DAG: `issue-66-daemon-path-context`

daemon 的路径上下文（home / work_dir / skill 扫描根）由外部调用方（TUI/controller）显式传入，
kernel 内部不再散读进程环境变量；统一 skill install 与 load 的目录契约；session 显式绑定 work_dir。
