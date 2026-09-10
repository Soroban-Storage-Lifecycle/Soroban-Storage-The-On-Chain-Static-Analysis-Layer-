# Soroban Storage Lifecycle Guide

Every persistent entry in a Soroban contract has a time-to-live (TTL) and needs
rent paid, or it gets evicted. Choosing the wrong storage type can permanently
destroy user funds. This guide explains the model and how `soroban-storage`
makes the correct pattern the default.

## The three storage types

```rust
env.storage().temporary()    // cheapest; Permanently DELETED on expiry
env.storage().persistent()   // archived on expiry; restorable
env.storage().instance()     // shares the contract instance TTL
```

| | Temporary | Persistent | Instance |
|---|---|---|---|
| Fee | Cheapest | Same as instance | Same as persistent |
| When TTL hits zero | **Permanently deleted. Cannot be restored.** | Archived; restored automatically when accessed (Protocol 23+) or via `RestoreFootprintOp` | Contract instance archived; auto-restored on use |
| Restored TTL | n/a | Network minimum (`current ledger + 4095`) | Network minimum |
| Capacity | Unlimited | Unlimited | Limited (single ledger entry) |
| Suitable for | Oracles, signatures, nonces — anything easily recreated or time-bounded | User data that cannot be recreated: **balances**, positions, grants | Shared contract metadata: admin, config, decimals |

> ⚠️ **The footgun:** storing a user balance in `temporary()` is irreversible
> data loss. Unlike persistent entries, there is no archive and no restore — the
> entry is gone forever. This is precisely the class of bug SDF shipped in
> production (an outdated copy of a persistent entry being moved into the hot
> archive), and both open SCF RFPs list TTL management as a mandatory hazard.

## TTL mechanics

- **TTL** = how many ledgers remain until the entry is no longer live
  (`live_until_ledger - current_ledger`).
- **Minimum TTL** — applied when an entry is created **or restored** (network
  parameter; restored persistent entries get ~`current ledger + 4095`).
- **Maximum TTL** — per lifecycle, a network parameter (persistent
  `~6_311_390` ledgers ≈ 1 year; temporary `~3_110_400` ≈ 180 days at ~5s
  ledgers). Extension requests above it are **silently clamped by the host**.
- **`extend_ttl(key, threshold, extend_to)`** — extends *only if* the current
  TTL is below `threshold`, then sets it to `extend_to`. This
  check-then-extend shape is what makes bump-on-access cheap.

Ledger arithmetic at the current ~5-second cadence: `1 day = 17_280 ledgers`.

## What the framework enforces

### 1. Explicit lifecycle per key (compile time)

Every entry in `storage!` declares exactly one lifecycle. The generated
accessors route all reads/writes through that lifecycle — there is no API that
lets a key be accessed through another one.

```rust
storage! {
    #[storage(persistent, critical)]
    Balance(Address) -> i128,
}
```

Declaring an entry without a lifecycle is a compile error:

```text
error: storage entry requires an explicit lifecycle: add `#[storage(persistent)]`,
       `#[storage(temporary)]`, or `#[storage(instance)]`
```

### 2. Critical data can never be temporary (compile time)

Mark entries `critical` when they hold funds / irreplaceable state. The macro
rejects `critical` + `temporary`:

```text
error: critical entry declared `temporary`: temporary entries are permanently
       deleted when their TTL expires and can never be restored. Use
       `persistent` (or `instance`) for data that cannot be recreated.
```

### 3. Auto-bump TTL on access

`get`, `set` and `update` bump the entry TTL to the entry's policy target
(`extend_to`) whenever the current TTL is below the policy threshold
(`threshold`). Defaults:

| Lifecycle | Re-bump when TTL < | Extend to |
|---|---|---|
| persistent / instance | 7 days | 30 days |
| temporary | 1 day | 30 days |

Policies are per-entry and const:

```rust
storage! {
    #[storage(persistent, critical)]
    #[storage(policy = TtlPolicy::days(7, 30))]  // threshold 7d, extend to 30d
    Balance(Address) -> i128,

    #[storage(temporary, no_bump)]               // opt out; manage TTL yourself
    Nonce(Address) -> u64,
}
```

### 4. Explicit TTLs for bounded data

`set_ttl(key, val, ttl_ledgers)` writes an entry and pins its lifetime — right
for claims, offers, grants:

```rust
// Valid for 14 days from the moment it is written.
GrantKey::set_ttl(&env, &user, &amount, 14 * LEDGERS_PER_DAY);
```

The explicit TTL overrides the policy bump (`extend_ttl(key, ttl, ttl)` — the
host only extends when below the threshold, so the result is
`max(current, ttl_ledgers)`).

## What the linter catches (raw `env.storage()` code)

The macro guarantees apply to code using the framework. Hand-rolled
`env.storage()` code is covered by `cargo soroban-lint`:

- `temporary_critical_type` — critical-looking types written to `.temporary()`
  (via `Critical` derives, `storage!` critical entries, or a naming heuristic).
- `lifecycle_mismatch` — a `storage!`-declared key used through a different
  lifecycle (each lifecycle is a separate key space, so this silently reads or
  writes the wrong entry).
- `missing_ttl_extension` — write paths with no `extend_ttl`/`bump` anywhere in
  the function, so entries drift toward expiry.
- `instance_storage_bloat` — unbounded collections or loop-written values in
  instance storage.
- `temporary_ttl_exceeded` — temporary extensions beyond the network maximum,
  which the host clamps silently.

See [lint-rules.md](lint-rules.md) for details.

## Design rules of thumb

1. **Balances, positions, claims, grants → persistent** (marked `critical`).
2. **Admin, decimals, metadata → instance** (small, fixed, shared).
3. **Nonces, quotes, signatures, oracle data → temporary** (recreatable /
   time-bounded). If it *could* be a user balance, it is not temporary.
4. **Everything you keep gets its TTL extended** — in-contract bump-on-access
   keeps *actively used* data alive; idle data needs an off-chain rent keeper
   (see `soroban-rent-keeper`) that submits `ExtendFootprintTTLOp` batches.

## Network parameters

The constants in [`crates/soroban-storage/src/ttl.rs`](../crates/soroban-storage/src/ttl.rs)
(`LEDGERS_PER_DAY`, `MIN_PERSISTENT_TTL`, `MAX_PERSISTENT_TTL`, `MAX_TEMP_TTL`)
reflect mainnet parameters and are labeled as such: they are network parameters
and can change via network upgrades. The host clamps extension requests to the
true protocol maxima, so exceeding them is safe but wasteful — which is exactly
what the `temporary_ttl_exceeded` lint warns about.

Current reference values (verify at [lab.stellar.org](https://lab.stellar.org/network-limits)):
restored persistent entries → `current ledger + 4095`; maximum temporary TTL ≈
`3_110_400` ledgers; maximum persistent TTL ≈ `6_311_390` ledgers.