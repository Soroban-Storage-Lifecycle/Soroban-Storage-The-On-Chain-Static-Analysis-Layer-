# Restore Planner

`soroban-restore-planner` recovers archived Soroban state. Persistent entries
whose TTL expired are moved out of live state; they are not lost, but they are
not usable until a `RestoreFootprintOp` brings them back. The planner figures
out *which* entries are archived and emits the transaction that restores them.

> Temporary entries cannot be restored — they are deleted permanently when
> their TTL reaches zero. This tool is for **persistent** entries.

## Usage

```bash
soroban-restore-planner \
  --contract C... \
  --source G... \
  --rpc-url https://soroban-testnet.stellar.org \
  --network-passphrase "Test SDF Network ; September 2015"
```

The unsigned transaction envelope is printed as base64 XDR. Sign it with any
wallet or submit it directly:

```bash
# Emit and sign in one step, then submit
soroban-restore-planner --contract C... --signer-secret S... --submit ...
```

| Option | Description |
|---|---|
| `--contract <C...>` | Contract id to restore (required in single mode) |
| `--config <file.json>` | Batch mode: restore every contract in the file |
| `--rpc-url <URL>` | Stellar RPC endpoint (or `SOROBAN_RPC_URL`) |
| `--network-passphrase <PASS>` | Must match the RPC network (or `SOROBAN_NETWORK_PASSPHRASE`) |
| `--source <G...>` | Transaction source account (or derived from `--signer-secret`) |
| `--signer-secret <S...>` | Secret key used to sign with `--submit` (or `SOROBAN_SIGNER_SECRET`) |
| `--key <BASE64_XDR>` | Extra contract-data `LedgerKey` to consider (repeatable) |
| `--wasm-hash <HEX>` | Contract code hash (64 hex chars); needed when the instance is archived and the hash cannot be resolved |
| `--submit` | Sign and submit (requires `--signer-secret`) |
| `--out <PATH>` | Write the unsigned envelope to a file |
| `--json` | Emit a JSON report |

Exit codes: `0` success, `1` runtime failure, `2` usage error.

## Batch mode

Restoring a fleet (a shared code hash, a whole deployment) one contract at a
time means scripting the CLI and parsing N outputs. `--config` takes a list
instead:

```json
{
  "rpc_url": "https://soroban-testnet.stellar.org",
  "network_passphrase": "Test SDF Network ; September 2015",
  "signer_secret": "S...",
  "submit": false,
  "contracts": [
    { "contract_id": "C..." },
    { "contract_id": "C...", "wasm_hash": "0101..." }
  ]
}
```

```bash
soroban-restore-planner --config contracts.json --out envelopes/
soroban-restore-planner --config contracts.json --json > results.json
```

The run continues past failures — one bad contract id does not stop the rest —
and exits non-zero if any contract failed. With `--json` the output is an array
of `{ contract, ok, report, error }`; in human mode each contract is printed and
a final `N contract(s): X ok, Y failed` summary line is emitted. `--out` is
treated as a **directory** in batch mode and each unsigned envelope is written
to `<out>/<contract_id>.xdr`.

Flags (`--rpc-url`, `--network-passphrase`, `--source`, `--signer-secret`,
`--submit`) override the corresponding config fields. A template lives at
[`crates/soroban-restore-planner/restore.example.json`](../crates/soroban-restore-planner/restore.example.json).

## How discovery works

A naive implementation would query `getLedgerEntries` and restore whatever
comes back. That does not work: `getLedgerEntries` only reports **live** state,
so an archived entry is simply absent — indistinguishable from one that never
existed.

The planner instead:

1. builds the **candidate** set — the contract instance key, the code key (when
   its hash can be resolved or was supplied), and any `--key` entries;
2. builds a **probe** `RestoreFootprintOp` transaction whose read-write
   footprint contains those candidates;
3. simulates it. When the footprint contains archived entries, the RPC returns
   the authoritative footprint in `restorePreamble`;
4. rebuilds the final transaction from the simulated `SorobanTransactionData`
   and resource fee.

The entries that appeared in the simulation's read-write footprint are the ones
reported as `archived`.

## Output

Human-readable:

```text
contract:        C...
current ledger:  1234567
candidates:      2
archived:        1
  - instance (AAAA...)
resource fee:    12345 stroops
unsigned transaction (base64 XDR):
AAAA...
```

JSON (`--json`):

```json
{
  "contract": "C...",
  "current_ledger": 1234567,
  "candidates": [{ "label": "instance", "key_xdr": "AAAA..." }],
  "archived": [{ "label": "instance", "key_xdr": "AAAA..." }],
  "min_resource_fee": 12345,
  "unsigned_transaction_xdr": "AAAA...",
  "submitted_tx_hash": null
}
```

## Verifying on testnet

```bash
export SOROBAN_RPC_URL=https://soroban-testnet.stellar.org
export SOROBAN_NETWORK_PASSPHRASE="Test SDF Network ; September 2015"
export SOROBAN_SIGNER_SECRET=S...
export SOROBAN_TEST_CONTRACT=C...
cargo test -p soroban-restore-planner --test testnet_smoke
```

The test self-skips unless all four variables are set. It verifies the
passphrase, resolves the candidates and produces a plan, printing how many
entries the network reports as archived.

To exercise a real restore: archive a throwaway contract by leaving it
untouched past its TTL, run the test and confirm `archived >= 1`, then run the
CLI with `--submit` and re-check that the instance entry is live again.
Submitting is deliberately **not** part of the automated smoke test — there is
no generic way to guarantee an archived target exists, and a restore with
nothing archived is not a meaningful assertion.

## Caveats

- **Signed with a network passphrase.** The signature covers
  `TransactionSignaturePayload` (the `sha256(passphrase)` network id plus the
  tagged transaction), not the bare transaction.
- **Restored entries get the network minimum TTL** (`current ledger + 4095`).
  Restoring makes the contract usable again; it does not pin a long lifetime.
  Pair it with the [rent keeper](rent-keeper.md) so the restored entries stay
  alive.
- **The network layer is not live-tested.** The simulate → build → sign →
  submit flow is unit- and fake-tested but has not been exercised against a
  live RPC in CI. Verify one restore on testnet first.
- **Unknown code hash.** If the instance itself is archived, the planner
  cannot read the wasm hash from it. Pass `--wasm-hash` to include the code
  entry; otherwise only the instance (and any explicit keys) are restored.
