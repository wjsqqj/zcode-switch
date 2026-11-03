# 发布 CI（Release workflow）契约

> 2026-11-04 实测沉淀。发版前先读这一页，能省掉两次「排队 15 分钟后被取消」。

## 触发与标记

- 只由 tag push 触发：`on.push.tags: ['v*']`。
- `prerelease: ${{ contains(github.ref_name, '-beta') }}`，其余版本 `makeLatest: 'true'` → 非 beta 的 tag 自动成为 **Latest**。

## macOS 矩阵：必须用固定镜像 `macos-14`

| 镜像 | 实测结果 |
|---|---|
| `macos-latest` | v1.7.4 / v1.7.5 共 **3 次**：job 排队 **15:00 整被取消**，`runner_name` 为空、`steps: []`（从未分配机器） |
| `macos-14` | 2 分钟内分配到 runner，正常构建 |

**契约**

- 两个 macOS job 都用 `macos-14`：
  - 原生 arm64：`bundles: dmg`
  - Intel：`bundles: dmg` + `rust_target: x86_64-apple-darwin`（交叉编译，不需要 Intel 机器）
- 不要用 `macos-13`（镜像已进入退役周期）。
- 账户是 **free plan + public repo**，Actions 不计费；`billing/usage` 为空是正常的，**不要**把排队误判成额度耗尽。

## 校验和：macOS 没有 GNU `sha256sum`

```bash
TGT="${{ matrix.rust_target }}"; [ -n "$TGT" ] || TGT="native"
if command -v sha256sum >/dev/null 2>&1; then HASH=(sha256sum); else HASH=(shasum -a 256); fi
find src-tauri/target -type f \( -name '*.dmg' -o -name '*.deb' -o -name '*.AppImage' \) -print0 \
  | xargs -0 "${HASH[@]}" | sed 's| .*/| |' > "checksums-${RUNNER_OS}-${TGT}-${GITHUB_REF_NAME}.txt"
gh release upload "$GITHUB_REF_NAME" "checksums-${RUNNER_OS}-${TGT}-${GITHUB_REF_NAME}.txt" --clobber
```

- 这一步失败**不影响产物**：`Build Tauri & upload release assets` 已把 dmg/exe 传上去，只是少一个 checksums 文件。
- 补救：本地 `gh release download` 下 dmg → 算 SHA-256 → 按同名规则上传（`--clobber`）。

## 重发同一版本（改 workflow / 补说明）

```bash
gh release delete vX.Y.Z --yes --cleanup-tag   # 删 release + 远端 tag
git tag -d vX.Y.Z
git tag -a vX.Y.Z -m "Z·SWITCH vX.Y.Z"
git push origin refs/tags/vX.Y.Z
```

- **tag 必须指向含最新 workflow 的提交**，否则重跑用的还是旧 workflow（改了 YAML 打旧 commit 的 tag 无效）。
- 版本号三处（`package.json` / `tauri.conf.json` / `Cargo.toml`）保持一致，`scripts/dist.mjs` 会校验。

## 排障命令

```bash
gh run list -L 3
gh run view <run_id> --json status,conclusion,jobs \
  --jq '{status,jobs:[.jobs[]|{name,status,conclusion}]}'
gh api repos/<owner>/<repo>/actions/runs/<run_id>/jobs \
  --jq '.jobs[]|select(.name|test("macos"))|{runner_name,started_at,completed_at}'
gh run view <run_id> --log-failed
```

- `runner_name` 为空 = 从未分配到机器（排队/调度问题），不是编译错误。
- `steps: []` + 恰好 15 分钟 = 典型的 macOS 池排队取消。
