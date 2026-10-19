# live2d-ai-mod-local-tts

本地 TTS 拉起 Mod（稳定 id local-tts-melo；工厂 `MELO_FACTORY`（缺省启用）+ `FACTORY`（`local-tts`，CosyVoice3，已封存））的实现 crate。本仓库是 2026-10-19 从主仓 `/home/skystar/Live2D-Ai` 拆出来的独立本地仓库：**仓库根 = crate 根**（`Cargo.toml` 在根），许可证 AGPL-3.0-only（见 `LICENSE`）。

依赖方向只有「Mod → 主仓」：`live2d-ai-mod-system` 走 path 指向同父目录的 `../Live2D-Ai/crates/...`；没有指向旧第二仓 `../Live2D-Ai-mods` 的 path，也没有 Mod→Mod 的 path 依赖。本仓是独立 workspace（`Cargo.toml` 末尾空的 `[workspace]`），可以 `cd` 进来单独跑门禁。

## 挂回主仓（维护者手动，**没有动态加载器**）

1. 主仓根 `Cargo.toml` 的 `[workspace.dependencies]` 加一行：`live2d-ai-mod-local-tts` = { path = "../live2d-ai-mod-local-tts" }；
2. `crates/live2d-ai-desktop/Cargo.toml` 加同名依赖（`live2d-ai-mod-local-tts` = { workspace = true }）；
3. 把 `MELO_FACTORY`（缺省启用）+ `FACTORY`（`local-tts`，CosyVoice3，已封存） 加进 `AVAILABLE_MOD_FACTORIES`；
4. 本 Mod 没有独立宿主路由；只按用户 argv spawn 外部进程（`melo/start.sh` / `engine/start.sh`），不写 `[tts]`。要缺省启用还得改主仓 `default_mods_manifest`。

## 权重不入库（先下载再启用）

本仓库**不含任何模型权重**，`melo/weights/` 与 `weights/` 都是本机下载目录（已在 `.gitignore`）：

```bash
# MeloTTS 中文（config.json + checkpoint.pth，约 208MB）
melo/download.sh
# CosyVoice3（源码缓存到仓库外 + 权重下到 weights/）
engine/download.sh
```

首次启用前必须先跑对应下载脚本；失败时 Mod 标 Failed，不重试、不改出声地址。许可证与模型卡见 `melo/LICENSE`、`melo/MODEL-CARD.md`、`engine/LICENSE`、`engine/NOTICE`、`engine/MODEL-CARD.md`。

## 门禁

```bash
cargo test --all-targets
cargo test --doc
cargo fmt -- --check
cargo clippy --all-targets -- -D warnings
```

远端未建、未 push；本地 `main` 分支一次提交。
