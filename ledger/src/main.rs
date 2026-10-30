//! mirrorleaf-ledger: the sync-status service (axum and Postgres). It holds device
//! lists, change counters and push tokens, never note content. Built in M3
//! (implementation plan §5.8–§5.9).

fn main() {
    println!("mirrorleaf-ledger {}", env!("CARGO_PKG_VERSION"));
}
