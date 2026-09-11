# Lint Rules Reference

`cargo soroban-lint` statically analyzes a contract's source without building
it. It is deliberately **conservative**: when a fact cannot be determined from
syntax alone, the rule stays quiet rather than risk a false positive. The
`storage!` macro provides the hard compile-time guarantees; these rules cover
raw `env.storage()` code and structural smells.

```text
cargo soroban-lint src --deny-warnings
```

| Rule | Severity | Exit code impact |
|---|---|---|
| `temporary_critical_type` | error | fails build |
| `lifecycle_mismatch` | error | fails build |
| `missing_ttl_extension` | warning | fails with `--deny-warnings` |
| `instance_storage_bloat` | warning | fails with `--deny-warnings` |
| `temporary_ttl_exceeded` | warning | fails with `--deny-warnings` |
| `persistent_ttl_exceeded` | warning | fails with `--deny-warnings` |

Exit codes: `0` clean (or warnings without `--deny-warnings`), `1` errors found
(or warnings with `--deny-warnings`), `2` usage error.

---

## `temporary_critical_type` (error)

**What it flags:** data that looks like user funds or irreplaceable state being
written through `env.storage().temporary().set/update/try_update(...)`.

**Why:** temporary entries are permanently deleted when their TTL expires and
can never be restored. A balance in temporary storage is a time bomb that
detonates into irreversible loss of funds.

**How it decides a value is critical** (first match wins, all syntax-only):

1. The key expression references a `storage!` entry declared `critical`
   (e.g. `env.storage().temporary().set(&StorageKey::Balance(u), &x)` where
   `Balance` is declared `critical`).
2. The value's type derives `Critical` in the crate
   (`#[derive(soroban_storage::Critical)]` or `impl Critical for X`).
3. The value's type matches the critical-data naming heuristic
   (`balance`, `amount`, `fund`, `collateral`, `position`, `ledger`, `escrow`,
   `vault`, `reserve`, `deposit`, `allowance`, `debt`, `credit`, `asset`,
   `token`, `share`, `reward`, `claim` — camel-case aware).

Value types are resolved from function parameter types, closure parameter types,
and field/method-name hints. When the type cannot be resolved, the rule stays
quiet (e.g. `set(&key, &compute(env))`).

```rust
// flagged: `balance: Balance` and `Balance` derives Critical
env.storage().temporary().set(&key, &balance);

// flagged: `amount: TokenBalance` matches the naming heuristic
env.storage().temporary().set(&key, &amount);

// not flagged: `nonce: u64` is a primitive, no critical hint
env.storage().temporary().set(&key, &nonce);
```

**Fix:** move the data to persistent storage (or instance storage for shared
metadata), ideally via the `storage!` macro which rejects this at compile time.

---

## `lifecycle_mismatch` (error)

**What it flags:** a `storage!`-declared key accessed through a different
lifecycle than the one it was declared with.

```rust
// `Balance` is declared `persistent`, but read through `.temporary()`
env.storage().temporary().get(&StorageKey::Balance(user));

// `Nonce` is declared `temporary`, but written through `.persistent()`
env.storage().persistent().set(&StorageKey::Nonce(user), &nonce);
```

**Why:** each lifecycle is a separate key space. Accessing a key through the
wrong lifecycle doesn't error — it silently reads or writes a *different*
entry, which is worse.

**Fix:** route accesses through the generated accessors
(`BalanceKey::get(...)`), which hard-wire the declared lifecycle.

---

## `missing_ttl_extension` (warning)

**What it flags:** a function containing a raw persistent/temporary
`.set(...)` / `.update(...)` / `.try_update(...)` with **no** TTL management
call (`extend_ttl`, `extend_ttl_with_limits`, `bump`, `set_ttl` — including
`soroban_storage::ops::*` forms) anywhere in the same function.

**Why:** entries whose TTL is never extended drift toward expiry. Persistent
entries get archived (restorable, but restored to the network minimum);
temporary entries are deleted outright.

**Limits (by design):**

- Function-level only. A write in `fn a` and a bump in `fn b` still warns —
  the intent is to push TTL management onto the write path.
- Framework accessors (`BalanceKey::set`, `ops::set`, ...) are auto-bumping by
  construction and are **not** flagged; this rule only fires on hand-rolled
  `env.storage()` chains.

```rust
// flagged: no TTL management in this function
env.storage().persistent().set(&key, &value);

// clean: extends in the same function
env.storage().persistent().set(&key, &value);
env.storage().persistent().extend_ttl(&key, 17_280, 518_400);

// clean: framework accessor auto-bumps
BalanceKey::set(&env, &user, &value);
```

---

## `instance_storage_bloat` (warning)

**What it flags:** instance-storage writes that grow the single contract
instance entry:

- values whose type is an unbounded collection (`Vec`, `Map`, `String`,
  `Bytes`, ...);
- values whose type is a struct with more than 3 fields;
- writes inside a `for` / `while` / `loop`.

**Why:** instance storage is one ledger entry whose size is paid on **every**
contract invocation. Bloating it raises the floor cost of every call and, in
the limit, the entry can exceed network limits.

**Fix:** put unbounded per-user data in persistent storage; keep instance
storage to small, fixed, shared metadata (admin, decimals, config). Prefer a
single bounded struct written once.

---

## `temporary_ttl_exceeded` (warning)

**What it flags:** `.temporary().extend_ttl(key, threshold, extend_to)` where
`extend_to` exceeds the network maximum for temporary entries (`3_110_400`
ledgers ≈ 180 days at ~5s ledgers).

**Why:** the host clamps the extension silently, so the code claims a lifetime
the network will not honor. The effective TTL is shorter than intended.

**Fix:** extend to at most the network maximum (check
`lab.stellar.org/network-limits` for current values — they are network
parameters and can change via upgrades).

---

## `persistent_ttl_exceeded` (warning)

**What it flags:** `.persistent().extend_ttl(key, threshold, extend_to)` or
`.instance().extend_ttl(...)` where `extend_to` exceeds the network maximum for
persistent/instance entries (`6_311_390` ledgers ≈ 1 year at ~5s ledgers).

**Why:** exactly the temporary case, one lifecycle over — the host clamps the
extension silently, so the code claims a lifetime the network will not honor,
and the resource fee for the excess buys nothing.

```rust
// flagged: beyond the persistent maximum
env.storage().persistent().extend_ttl(&key, 0, 7_000_000);

// fine: within the network maximum
env.storage().persistent().extend_ttl(&key, 17_280, 518_400);
```

**Fix:** extend to at most the network maximum (check
`lab.stellar.org/network-limits` for current values — they are network
parameters and can change via upgrades).

---

## Configuration

```text
cargo soroban-lint [PATHS]... [--deny-warnings] [--json] [--ignore RULE]
```

- `PATHS` — directories (recursively scanned for `.rs`) or files. Default:
  `./src` if it exists, else `.`.
- `--deny-warnings` — warnings fail the run (exit 1).
- `--json` — emit findings as a JSON array.
- `--ignore RULE` — skip a rule by name (repeatable).

In CI, findings are emitted as GitHub Actions annotations
(`::error file=...,line=...,col=...::`) automatically when
`GITHUB_ACTIONS=true`.

## A note on "why not a clippy lint?"

Custom clippy lints require loading an external dylib into the clippy driver
(`--load-from`), a mechanism **removed from modern clippy**. The maintained,
production-grade equivalent for ecosystem-specific rules is a cargo subcommand
(`cargo soroban-lint`), which works on any stable toolchain, runs without
building the contract, and drops into CI in one step. The rules implemented
here are the static-analysis rules described in the project brief — temporary
with critical types, missing TTL extensions, instance-storage bloat — plus two
adjacent safety nets (lifecycle mismatch, TTL over-extension).