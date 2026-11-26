# Typed TypeScript Bindings (`forge bindings`)

FORGE preserves a zero-macro Rust interface (`async fn` handlers without procedural macros), while providing `forge bindings` to generate compile-time checked TypeScript command names and declaration-mergeable payload/result types.

---

## 1. Purpose & User-Facing Behavior

Running `forge bindings` executes your FORGE binary in `FORGE_EXPORT_COMMANDS` mode (zero window creation) and writes a `.d.ts` file containing:
- `ForgeCommandName` — Exact union type of every registered user, plugin, and built-in command (`"greet" | "fs.read_text" | "window.close" | ...`).
- `ForgeCommandTypes` — Open TypeScript interface you can augment in your own code to bind strongly typed payloads and return values to any command name.
- `ForgeBridge` & `ForgeWindowBridge` — Complete typings for `window.forge.*`.

---

## 2. CLI Interface

```bash
# Writes `src-ui/forge-commands.d.ts` (or `ui/forge-commands.d.ts`)
forge bindings

# Custom output path
forge bindings --out src-ui/types/forge-commands.d.ts
```

---

## 3. Minimal Usage Example

After running `forge bindings`, augment `ForgeCommandTypes` in any `.d.ts` or `.ts` file inside `src-ui/`:

```ts
declare module "./forge-commands" {
  interface ForgeCommandTypes {
    greet: {
      payload: { name: string };
      result: { message: string; runtime: string };
    };
  }
}

// `res` is automatically inferred as `{ message: string; runtime: string }`,
// and any typo in `"greet"` or `{ name }` fails `tsc --noEmit` immediately!
const res = await window.forge.invoke("greet", { name: "Ada" });
```

---

## 4. Optional `ts-rs` / `schemars` Recipe

If you want Rust structs to emit TypeScript interfaces automatically alongside `forge bindings`, annotate your payload/response structs with `#[derive(ts_rs::TS)]` in a cargo test and reference the generated types inside your `ForgeCommandTypes` augmentation—no changes to FORGE's handler signatures are required.
