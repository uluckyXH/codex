"""只读取教程与源码定义，不创建用户配置，不执行认证或网络请求。"""

import json
from pathlib import Path
import re
import subprocess
import tomllib

root = Path.cwd()
doc = root / "docs/鸿蒙电脑原生适配/接口密钥安装速用.md"
delivery = root / ".harmony-build/交付/鸿蒙Codex安装/接口密钥安装速用.md"
assert doc.read_bytes() == delivery.read_bytes(), "交付副本不一致"
schema = json.loads((root / "codex-rs/core/config.schema.json").read_text())
shell_count = 0
configs = []
for language, body in re.findall(r"```([^\n]+)\n(.*?)```", doc.read_text(), re.S):
    if language not in {"sh", "zsh"}:
        continue
    command = ["/bin/zsh", "-f", "-n"] if language == "zsh" else ["/bin/sh", "-n"]
    subprocess.run(command, input=body, text=True, check=True)
    shell_count += 1
    configs.extend(tomllib.loads(text) for text in re.findall(r"<<'TOML'\n(.*?)\nTOML", body, re.S))

assert len(configs) == 2
for config in configs:
    assert set(config) <= set(schema["properties"]), "存在源码未定义的顶层配置字段"
    assert config["cli_auth_credentials_store"] == "file"
    assert config["approval_policy"] == "untrusted"
    assert config["sandbox_mode"] == "workspace-write"
custom = configs[1]
for field, definition in {
    "approvals_reviewer": "ApprovalsReviewer",
    "web_search": "WebSearchMode",
    "personality": "Personality",
}.items():
    assert custom[field] in schema["definitions"][definition]["enum"]
assert custom["model"] == "gpt-5.6-terra"
assert custom["service_tier"] == "default"
assert custom["model_reasoning_effort"] == "max"
assert custom["plan_mode_reasoning_effort"] == "xhigh"
assert custom["model_provider"] == "proxy"
provider = custom["model_providers"][custom["model_provider"]]
assert set(provider) <= set(schema["definitions"]["ModelProviderInfo"]["properties"])
assert provider["name"] == "OpenAI"
assert provider["wire_api"] == "responses"
assert provider["requires_openai_auth"] is False
assert provider["experimental_bearer_token"] == "这里填写你的API Key"
assert provider["base_url"] == "你获得的那个地址"
assert "env_key" not in provider
assert "HARMONY_CODEX_API_KEY" not in doc.read_text()
print(f"接口教程 {shell_count} 段 Shell/zsh 语法通过；{len(configs)} 段 TOML 解析通过。")
print("字段名和所检查枚举与仓库配置定义一致；proxy、固定 token 占位及用户指定参数一致。")
print("交付副本与仓库教程一致；没有执行示例命令、登录、凭据读写或模型请求。")
