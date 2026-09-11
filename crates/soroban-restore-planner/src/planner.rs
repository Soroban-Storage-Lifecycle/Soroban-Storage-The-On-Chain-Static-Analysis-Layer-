//! Orchestration: resolve candidate entries, probe the network, and build the
//! final `RestoreFootprintOp` transaction.
//!
//! The [`Planner`] is generic over [`Rpc`], so the whole flow is exercised in
//! tests with a fake that returns canned ledger state.

use stellar_xdr as xdr;

use crate::keys::account_strkey;
use crate::plan::{self, Candidate, PlannedRestore, Simulation};
use crate::rpc::Rpc;

/// Builds restore plans for a configured source account.
pub struct Planner<R: Rpc> {
    rpc: R,
    source: xdr::AccountId,
}

impl<R: Rpc> Planner<R> {
    pub fn new(rpc: R, source: xdr::AccountId) -> Self {
        Planner { rpc, source }
    }

    /// Plans the restoration of `candidates`: probes the network with a
    /// `RestoreFootprintOp` carrying the candidate footprint, then rebuilds the
    /// transaction from the simulated resource data.
    ///
    /// Archived candidate entries are reported in [`PlannedRestore::archived`];
    /// when nothing is archived the returned transaction is still valid and
    /// simply restores nothing.
    pub async fn plan(&self, candidates: Vec<Candidate>) -> Result<PlannedRestore, String> {
        let current_ledger = self.rpc.latest_ledger().await?;
        let sequence = self
            .rpc
            .account_sequence(&account_strkey(&self.source))
            .await?;

        let footprint = plan::candidate_footprint(&candidates)?;
        let probe = plan::build_restore_transaction(
            &self.source,
            sequence,
            current_ledger,
            plan::placeholder_resources(footprint),
            100,
        )?;

        let mut simulation = self.rpc.simulate_restore(&probe).await?;
        let archived = select_archived(&candidates, &simulation)?;

        // A Soroban transaction fee is the inclusion fee plus the resource fee;
        // the resource fee is carried inside the transaction data as well.
        let fee = (simulation.min_resource_fee + 100).min(u32::MAX as u64) as u32;
        simulation.transaction_data.resource_fee =
            i64::try_from(simulation.min_resource_fee).unwrap_or(i64::MAX);

        let transaction = plan::build_restore_transaction(
            &self.source,
            sequence,
            current_ledger,
            simulation.transaction_data.clone(),
            fee,
        )?;
        let unsigned_xdr = plan::encode_unsigned(&transaction)?;

        Ok(PlannedRestore {
            current_ledger,
            candidates,
            archived,
            min_resource_fee: simulation.min_resource_fee,
            transaction,
            unsigned_xdr,
        })
    }

    /// Submits an already-signed envelope via the network.
    pub async fn submit(&self, envelope_base64: &str) -> Result<String, String> {
        self.rpc.submit_envelope(envelope_base64).await
    }
}

/// Keeps only the candidates whose key appears in the simulation's read-write
/// footprint (the entries the network says are archived).
fn select_archived(
    candidates: &[Candidate],
    simulation: &Simulation,
) -> Result<Vec<Candidate>, String> {
    let keys = plan::archived_keys(&simulation.transaction_data)?;
    Ok(candidates
        .iter()
        .filter(|c| keys.contains(&c.key_xdr))
        .cloned()
        .collect())
}
