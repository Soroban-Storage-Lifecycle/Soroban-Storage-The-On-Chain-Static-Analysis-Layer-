# Rent Keeper

`soroban-rent-keeper` is the off-chain half of the storage lifecycle story.
The `soroban-storage` framework keeps *actively used* entries alive by bumping
their TTL on every access. Data that nobody touches — dormant balances, old
grants, a contract that has been quiet for a while — still drifts toward
expiry. The rent keeper watches deployed contracts over Stellar RPC and
submits batched `ExtendFootprintTTLOp` transactions before that happens.

> Persistent entries are archived when they expire and can be restored;
> **temporary entries are permanently deleted**. The keeper only buys you time;
> the durability of the data is still decided by the lifecycle you chose at
> compile time.

## How it works

Every poll interval the keeper:

1. reads the latest ledger from RPC,
2. fetches the watched entries and their `live_until_ledger` (the TTL lives on
   the ledger entry key, not inside the entry),
3. classifies each entry by remaining TTL,
4. batches entries whose TTL is at or below the extension threshold and submits
   one `ExtendFootprintTTLOp` per batch,
5. exposes Prometheus metrics.

The pure risk/batching logic lives in [`crates/soroban-rent-keeper/src/risk.rs`](../crates/soroban-rent-keeper/src/risk.rs)
and is unit-tested; the network layer is behind the `Rpc` trait so the poll
loop is tested against fakes.

### Risk windows

| Level | Condition | Meaning |
|---|---|---|
| `Safe` | `ttl >= extend_to_ledgers` | the policy target is already met |
| `Watch` | `threshold < ttl < extend_to` | inside the envelope; extended by the normal cadence |
| `Critical` | `ttl <= threshold` | extended on this poll |

## Configuration

The daemon reads a JSON file passed with `--config`:

```json
{
  "rpc_url": "https://soroban-testnet.stellar.org",
  "network_passphrase": "Test SDF Network ; September 2015",
  "signer_secret": "S...",
  "metrics_addr": "127.0.0.1:9090",
  "max_batch_bytes": 200000,
  "poll_interval_secs": 600,
  "min_signer_balance_stroops": 10000000,
  "contracts": [
    {
      "contract_id": "C...",
      "threshold_ledgers": 172800,
      "extend_to_ledgers": 518400,
      "data_keys": ["AAAA..."]
    }
  ]
}
```

| Field | Default | Description |
|---|---|---|
| `rpc_url` | — | Stellar RPC endpoint (overridden by `SOROBAN_RPC_URL`) |
| `network_passphrase` | — | Must match the RPC network |
| `signer_secret` | — | `S...` key that pays for extensions (overridden by `SOROBAN_SIGNER_SECRET`) |
| `metrics_addr` | `127.0.0.1:9090` | `host:port` for `/metrics` |
| `max_batch_bytes` | `200000` | Upper bound on one extension footprint |
| `poll_interval_secs` | `600` | Seconds between polls |
| `allow_short_threshold` | `false` | Opt out of the threshold-vs-poll-cadence check |
| `min_signer_balance_stroops` | `10000000` (1 XLM) | Startup floor for the signer balance |
| `contracts[].contract_id` | — | `C...` contract id |
| `contracts[].threshold_ledgers` | `172800` (~10 days) | Extend when TTL drops to this |
| `contracts[].extend_to_ledgers` | `518400` (~30 days) | Extend to at least this |
| `contracts[].data_keys` | `[]` | Extra contract-data `LedgerKey`s (base64 XDR) |

The contract **instance** entry and its **code** entry are always watched; the
code key is resolved from the instance's wasm hash at startup. `data_keys` is
for contract-data entries you want to pin explicitly.

Never commit `signer_secret`; prefer `SOROBAN_SIGNER_SECRET` in the
environment.

A template lives at
[`crates/soroban-rent-keeper/config.example.json`](../crates/soroban-rent-keeper/config.example.json).

## Running

```bash
cargo run -p soroban-rent-keeper -- --config config.json
```

The process verifies the RPC network passphrase at startup and exits non-zero
on a mismatch or if it cannot resolve a contract's wasm hash.

## Metrics

Served as Prometheus text format on `http://<metrics_addr>/metrics`:

| Metric | Labels | Meaning |
|---|---|---|
| `soroban_rent_keeper_ttl_ledgers` | `contract`, `entry` | remaining TTL of a watched entry |
| `soroban_rent_keeper_extensions_total` | `contract`, `outcome` | submissions by outcome (`submitted`/`noop`/`error`) |
| `soroban_rent_keeper_last_poll_timestamp` | `contract` | unix time of the last poll |
| `soroban_rent_keeper_entries_watched` | `contract` | entries watched per contract |

Alert on `extensions_total{outcome="error"}` and on
`ttl_ledgers` approaching zero (the keeper should have refreshed it long
before then).

## Verifying on testnet

The suite ships opt-in smoke tests that self-skip (and pass) unless all of
these are set:

| Variable | Purpose |
|---|---|
| `SOROBAN_RPC_URL` | RPC endpoint |
| `SOROBAN_NETWORK_PASSPHRASE` | Network passphrase |
| `SOROBAN_SIGNER_SECRET` | Funded `S...` account |
| `SOROBAN_TEST_CONTRACT` | A deployed `C...` contract to watch |
| `SOROBAN_TESTNET_WRITE` | Set to `1` to allow the fee-spending extend test |

```bash
export SOROBAN_RPC_URL=https://soroban-testnet.stellar.org
export SOROBAN_NETWORK_PASSPHRASE="Test SDF Network ; September 2015"
export SOROBAN_SIGNER_SECRET=S...
export SOROBAN_TEST_CONTRACT=C...
cargo test -p soroban-rent-keeper --test testnet_smoke
```

`testnet_read_smoke` verifies the passphrase, resolves the watch keys and
prints each entry's TTL. `testnet_extend_smoke` forces one
`ExtendFootprintTTLOp` and submits it, and only runs when
`SOROBAN_TESTNET_WRITE=1` — no funds are spent by accident. Confirm success by
re-running the read test and seeing the extended entries sit near the 30-day
policy target.

## Operational notes

- **Fund the signer.** Extensions cost the resource fee reported by
  simulation plus the inclusion fee. The startup floor catches an *empty*
  account, but it does not know your run-rate, so still alert on
  `extensions_total{outcome="error"}` and watch the balance over time.
- **One keeper per signer.** Concurrent keepers sharing a source account race
  on the sequence number; run a single instance or give each its own account.
- **Threshold vs. poll cadence is enforced.** At ~5s per ledger, a
  `poll_interval_secs` of 600 means ~120 ledgers elapse between polls. A
  contract with `threshold_ledgers < 120` can have an entry cross from *Safe*
  to *expired* between two polls, so config load fails with a clear message;
  set `allow_short_threshold: true` only if you have an out-of-band reason.
- **Signer balance floor.** The daemon checks the signer's balance at startup
  and refuses to run below `min_signer_balance_stroops` (default 1 XLM), rather
  than ticking along reporting `error` metrics while entries expire.
- **The network layer is not live-tested.** The build/simulate/sign/submit
  flow follows the documented pattern but has only been exercised against
  fakes. Verify one extension on testnet before trusting it with mainnet
  funds.
