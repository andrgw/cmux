//! Same seeded daemon and wire handler for old full-list and narrow app reads.
use super::*;
use std::time::Instant;

async fn read(hub: &Arc<Hub>, method: &str, params: Value) -> (Value, usize, f64) {
    let (tx, input) = tokio::sync::mpsc::channel(8);
    let (output, mut rx) = tokio::sync::mpsc::channel(8);
    let task = tokio::spawn(crate::server::serve_connection(hub.clone(), input, output));
    let start = Instant::now();
    tx.send(crate::rpc::Message::request(1, "initialize", json!({"protocolVersion": 1})).to_line())
        .await
        .unwrap();
    tx.send(crate::rpc::Message::request(2, method, params).to_line()).await.unwrap();
    let result = loop {
        let line = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        let frame: Value = serde_json::from_str(&line).unwrap();
        if frame["id"] == 2 {
            assert!(frame.get("error").is_none(), "{frame}");
            break (frame["result"].clone(), line.len(), start.elapsed().as_secs_f64() * 1000.0);
        }
    };
    task.abort();
    let _ = task.await;
    result
}

#[tokio::test]
async fn seeded_narrow_reads_exclude_peers_and_keep_closed_links() {
    let mut cfg = crate::config::Config::default();
    cfg.store.mode = crate::config::StoreMode::Memory;
    let store = crate::store::open(&cfg.store, std::path::Path::new("/nonexistent")).unwrap();
    for i in 0..200 {
        let meta = json!({"schema": "acpmux.session.v1", "id": format!("local-{i}"),
            "name": format!("local-{i}"), "harness": "claude", "cwd": "/work",
            "status": if i % 2 == 0 {"closed"} else {"idle"}, "createdAt": 1, "updatedAt": i + 1});
        store.save(&serde_json::from_value(meta).unwrap()).unwrap();
    }
    let hub = Hub::new(cfg, store);
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let peer = crate::peer::Peer::new("remote", "ws://127.0.0.1:9", None, None, tx);
    hub.peers.lock().unwrap().insert("remote".into(), peer.clone());
    let rows = (0..2800)
        .map(|i| {
            json!({"sessionId": format!("remote-{i}"),
        "name": format!("remote-{i}"), "harness": "claude", "cwd": "/remote/work",
        "status": if i % 2 == 0 {"closed"} else {"idle"}, "updatedAt": i + 201,
        "tools": [{"id": "tool", "description": "x".repeat(4096)}]})
        })
        .collect();
    hub.apply_peer_notice("remote", peer.generation, crate::peer::PeerNotice::Sessions(rows));
    // Warm the shared snapshot equally for both paths. No agents or peers are started.
    let _ = read(&hub, "_acpmux/sessions", json!({"limit": 1})).await;
    let (full, before_bytes, before_ms) = read(&hub, "_acpmux/sessions", json!({})).await;
    assert_eq!(full["sessions"].as_array().unwrap().len(), 3000);
    assert_eq!(
        full["sessions"].as_array().unwrap().iter().filter(|s| s["status"] == "closed").count(),
        1500
    );
    let mut params = json!({"localOnly": true, "excludeStatus": ["closed"], "orderBy": "sessionId", "limit": 64});
    let (mut count, mut bytes, mut ms, mut pages) = (0, 0, 0.0, 0);
    loop {
        let (page, size, time) = read(&hub, "_acpmux/sessions", params.clone()).await;
        pages += 1;
        bytes += size;
        ms += time;
        let rows = page["sessions"].as_array().unwrap();
        assert!(rows.iter().all(|r| r["peer"].is_null() && r["status"] != "closed"));
        count += rows.len();
        if page["nextCursor"].is_null() {
            break;
        }
        assert!(pages < 3, "quit reads must not page through the peer mirror");
        params["cursor"] = page["nextCursor"].clone();
    }
    assert_eq!(count, 100);
    assert_eq!(pages, 2);
    println!("PERF census bytes={before_bytes}->{bytes} ms={before_ms:.3}->{ms:.3} pages={pages}");
    let (_, watch_bytes, watch_ms) = read(&hub, "_acpmux/watch", json!({"enabled": true})).await;
    let (exact, exact_bytes, exact_ms) =
        read(&hub, "_acpmux/resolve", json!({"exactSessionId": true, "sessionId": "remote-0"}))
            .await;
    assert_eq!(exact["session"]["status"], "closed");
    assert_eq!(exact["session"]["cwd"], "/remote/work");
    assert_eq!(exact["session"]["peer"], "remote");
    println!("PERF link bytes={before_bytes}->{exact_bytes} ms={before_ms:.3}->{exact_ms:.3}");
    println!("PERF folder bytes={watch_bytes}->{exact_bytes} ms={watch_ms:.3}->{exact_ms:.3}");
    let (_, narrow_bytes, narrow_ms) =
        read(&hub, "_acpmux/watch", json!({"enabled": true, "limit": 1})).await;
    println!(
        "PERF harness-watch bytes={watch_bytes}->{narrow_bytes} ms={watch_ms:.3}->{narrow_ms:.3}"
    );
}

/// A hub on a memory store with `closed` closed chats, then `idle` idle ones, the closed ones
/// newest (each a millisecond apart): a page from the top is closed rows.
fn seeded(closed: usize, idle: usize) -> Arc<Hub> {
    let mut cfg = crate::config::Config::default();
    cfg.store.mode = crate::config::StoreMode::Memory;
    let store = crate::store::open(&cfg.store, std::path::Path::new("/nonexistent")).unwrap();
    for i in 0..closed + idle {
        let status = if i < idle { "idle" } else { "closed" };
        let meta = json!({"schema": "acpmux.session.v1", "id": format!("s-{i:05}"),
            "name": format!("chat-{i}"), "harness": "claude", "cwd": "/work",
            "status": status, "createdAt": 1, "updatedAt": 1_000 + i});
        store.save(&serde_json::from_value(meta).unwrap()).unwrap();
    }
    Hub::new(cfg, store)
}

/// Leo 2026-11-11, 11,236 chats (10,874 closed): `acpmux ls --limit 5` took 5.4 s, as long as
/// `--limit 1000`, because a page was cut from a list of every chat's row. A page of five builds
/// at most its five rows, at 500 chats and at 5,000, and so does the next page and a watch's
/// first page.
#[tokio::test]
async fn a_page_of_five_builds_its_five_rows_not_every_chat() {
    use std::sync::atomic::Ordering::SeqCst;
    let mut times = Vec::new();
    for n in [500_usize, 5_000] {
        let hub = seeded(n - n / 20, n / 20);
        let built = || hub.closed_summary_builds.load(SeqCst);
        let before = built();
        let (first, _, ms) = read(&hub, "_acpmux/sessions", json!({"limit": 5})).await;
        let rows = first["sessions"].as_array().unwrap();
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0]["sessionId"], format!("s-{:05}", n - 1));
        assert!(rows.iter().all(|r| r["status"] == "closed"));
        let first_built = built() - before;
        assert!(first_built <= 5, "{n} chats: a page of 5 built {first_built} closed rows");
        let before = built();
        let (second, _, _) = read(
            &hub,
            "_acpmux/sessions",
            json!({"limit": 5, "cursor": first["nextCursor"].clone()}),
        )
        .await;
        assert_eq!(second["sessions"][0]["sessionId"], format!("s-{:05}", n - 6));
        let second_built = built() - before;
        assert!(second_built <= 5, "{n} chats: the next page built {second_built} closed rows");
        let before = built();
        let (watch, _, _) = read(&hub, "_acpmux/watch", json!({"enabled": true, "limit": 5})).await;
        assert_eq!(watch["sessions"].as_array().unwrap().len(), 5);
        let watch_built = built() - before;
        assert!(watch_built <= 5, "{n} chats: a watch's page of 5 built {watch_built} closed rows");
        times.push((n, ms));
    }
    println!("PERF page-of-5 {times:?} (chats, ms)");
}
