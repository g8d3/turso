//! exact ddmin triple
use crate::common::TempDatabase;
use std::sync::Arc;
use turso_core::Connection;

#[test]
fn av_replay_seed_20002579() {
    let tmp_db = TempDatabase::builder()
        .with_opts(
            turso_core::DatabaseOpts::new()
                .with_index_method(true)
                .with_autovacuum(true)
                .with_attach(true)
                .with_encryption(true)
                .with_generated_columns(true),
        )
        .with_db_name("av_replay_20002579.db")
        .build();
    let mut connections: Vec<Arc<Connection>> = Vec::new();
    for _ in 0..10 {
        connections.push(tmp_db.connect_limbo());
    }

    let _ = connections[3].execute(r#"PRAGMA auto_vacuum=full;"#);
    let _ = connections[7].execute(r#"PRAGMA auto_vacuum=none;"#);
    let _ = connections[3].execute(r#"CREATE TABLE rousing_artnoose_8958 (giving_oikonomidou_8959 INTEGER PRIMARY KEY, fearless_stew_8960 TEXT, vivacious_leeder_8961 TEXT UNIQUE, excellent_staudenmaier_8962 INTEGER, twinkling_authors_8963 BLOB);"#);
}
