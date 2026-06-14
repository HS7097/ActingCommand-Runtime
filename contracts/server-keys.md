# Server Variant Keys

`server` is a backend-specific variant key, not a UI display suffix.

The UI may display short labels such as `Azur.jp`, `Ark.cn`, or `BA.gb`, but the database and runtime contracts must keep enough detail to route to the correct upstream resource and automation variant.

## Initial keys

| Game | Upstream | Key | Meaning |
| --- | --- | --- | --- |
| Azur | Alas | `alas.cn` | Alas `cn` |
| Azur | Alas | `alas.en` | Alas `en` |
| Azur | Alas | `alas.jp` | Alas `jp` |
| Azur | Alas | `alas.tw` | Alas `tw` |
| BA | BAAS | `baas.jp` | BAAS `jp` |
| BA | BAAS | `baas.cn` | BAAS `cn` |
| BA | BAAS | `baas.global_en` | BAAS `global_en` |
| BA | BAAS | `baas.ko` | BAAS `ko` |
| BA | BAAS | `baas.zh_tw` | BAAS `zh_tw` |
| Ark | MAA | `maa.bilibili` | MAA B server |
| Ark | MAA | `maa.official` | MAA official CN server |
| Ark | MAA | `maa.txwy` | MAA txwy server |
| Ark | MAA | `maa.yostar_en` | MAA YoStarEN |
| Ark | MAA | `maa.yostar_jp` | MAA YoStarJP |
| Ark | MAA | `maa.yostar_kr` | MAA YoStarKR |

## Rules

- New keys should be lowercase ASCII.
- Prefer `<upstream>.<variant>` unless a future backend needs a more specific namespace.
- Do not use `.jp`, `.cn`, or `.gb` alone in persisted runtime data.
- Do not add narrow database `CHECK` constraints for server keys. Use `server_variants` as the catalog.
- Keep user-facing display labels separate from persisted server keys.

