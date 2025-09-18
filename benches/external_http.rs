use axum::{routing::post, Json, Router};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use reqwest::Client;
use sentra::{app, build_state_from_env};
use serde_json::json;
use std::fs;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::runtime::Runtime;

fn bench_scenarios(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    // Shared runtime for all async operations
    let allow_handle = rt.block_on(async {
        async fn allow(Json(_v): Json<serde_json::Value>) -> Json<serde_json::Value> {
            Json(json!({"block": false}))
        }
        let app = Router::new().route("/ext", post(allow));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{}", addr);
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        url
    });
    let block_handle = rt.block_on(async {
        async fn block(Json(_v): Json<serde_json::Value>) -> Json<serde_json::Value> {
            Json(json!({"block": true}))
        }
        let app = Router::new().route("/ext", post(block));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{}", addr);
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        url
    });
    let timeout_handle = rt.block_on(async {
        async fn too_slow(Json(_v): Json<serde_json::Value>) -> Json<serde_json::Value> {
            tokio::time::sleep(Duration::from_millis(80)).await;
            Json(json!({"block": false}))
        }
        let app = Router::new().route("/ext", post(too_slow));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{}", addr);
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        url
    });
    let slow_handle = rt.block_on(async {
        async fn slow(Json(_v): Json<serde_json::Value>) -> Json<serde_json::Value> {
            tokio::time::sleep(Duration::from_millis(30)).await;
            Json(json!({"block": false}))
        }
        let app = Router::new().route("/ext", post(slow));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{}", addr);
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        url
    });

    // Prepare config file path helper
    let body = json!({"plannerContext":{"userMessage":"ping"},"toolDefinition":{"name":"SendEmail"},"inputValues":{}});

    // Prepare a dedicated Sentra instance (with its own plugin config) bound to a random port.
    // Returns the full analyze endpoint URL. Done once per scenario to keep per-iteration cost focused on request + plugin path.
    fn prepare(rt: &Runtime, ext_url: &str, plugin_name: &str) -> String {
        rt.block_on(async {
            let cfg = json!({"externalHttp":[{"name":plugin_name,"url":format!("{}/ext", ext_url),"reasonCode":900,"failOpen":false}]});
            let cfg_path = tempfile::NamedTempFile::new().unwrap();
            fs::write(cfg_path.path(), serde_json::to_string(&cfg).unwrap()).unwrap();
            std::env::set_var("SENTRA_PLUGIN_CONFIG", cfg_path.path().to_string_lossy().to_string());
            std::env::set_var("SENTRA_PLUGINS", plugin_name);
            let state = build_state_from_env().await.unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let app = app(state);
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
            format!("http://{}/analyze-tool-execution?api-version=2025-05-01", addr)
        })
    }

    let allow_url = prepare(&rt, &allow_handle, "external_bench_allow");
    let block_url = prepare(&rt, &block_handle, "external_bench_block");
    let slow_url = prepare(&rt, &slow_handle, "external_bench_slow");
    // For timeout we set failOpen true and a tight timeoutMs so request returns quickly after client timeout handling.
    let timeout_url = rt.block_on(async {
        let cfg = json!({"externalHttp":[{"name":"external_bench_timeout","url":format!("{}/ext", timeout_handle),"reasonCode":901,"failOpen":true,"timeoutMs":20}]});
        let cfg_path = tempfile::NamedTempFile::new().unwrap();
        fs::write(cfg_path.path(), serde_json::to_string(&cfg).unwrap()).unwrap();
        std::env::set_var("SENTRA_PLUGIN_CONFIG", cfg_path.path().to_string_lossy().to_string());
        std::env::set_var("SENTRA_PLUGINS", "external_bench_timeout");
        let state = build_state_from_env().await.unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = app(state);
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
        format!("http://{}/analyze-tool-execution?api-version=2025-05-01", addr)
    });

    // Fail-closed timeout scenario (same slow external, but failOpen=false so we expect block path on timeout).
    let timeout_fail_closed_url = rt.block_on(async {
        let cfg = json!({"externalHttp":[{"name":"external_bench_timeout_closed","url":format!("{}/ext", timeout_handle),"reasonCode":902,"failOpen":false,"timeoutMs":20}]});
        let cfg_path = tempfile::NamedTempFile::new().unwrap();
        fs::write(cfg_path.path(), serde_json::to_string(&cfg).unwrap()).unwrap();
        std::env::set_var("SENTRA_PLUGIN_CONFIG", cfg_path.path().to_string_lossy().to_string());
        std::env::set_var("SENTRA_PLUGINS", "external_bench_timeout_closed");
        let state = build_state_from_env().await.unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = app(state);
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
        format!("http://{}/analyze-tool-execution?api-version=2025-05-01", addr)
    });

    // Multi-external chain: three distinct fast allow externals executed sequentially.
    let multi_chain_url = rt.block_on(async {
        // Spin up three fast allow services
        async fn allow(Json(_v): Json<serde_json::Value>) -> Json<serde_json::Value> { Json(json!({"block": false})) }
        let mut endpoints = Vec::new();
        for _ in 0..3 {
            let app = Router::new().route("/ext", post(allow));
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let url = format!("http://{}", addr);
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
            endpoints.push(url);
        }
        let cfg = json!({"externalHttp":[
            {"name":"external_chain_a","url":format!("{}/ext", endpoints[0]),"reasonCode":910,"failOpen":true},
            {"name":"external_chain_b","url":format!("{}/ext", endpoints[1]),"reasonCode":911,"failOpen":true},
            {"name":"external_chain_c","url":format!("{}/ext", endpoints[2]),"reasonCode":912,"failOpen":true}
        ]});
        let cfg_path = tempfile::NamedTempFile::new().unwrap();
        fs::write(cfg_path.path(), serde_json::to_string(&cfg).unwrap()).unwrap();
        std::env::set_var("SENTRA_PLUGIN_CONFIG", cfg_path.path().to_string_lossy().to_string());
        std::env::set_var("SENTRA_PLUGINS", "external_chain_a,external_chain_b,external_chain_c");
        let state = build_state_from_env().await.unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = app(state);
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
        format!("http://{}/analyze-tool-execution?api-version=2025-05-01", addr)
    });

    let client = Client::new();

    let mut run_case = |label: &str, analyze_url: &str| {
        c.bench_function(label, |b| {
            b.iter_custom(|iters| {
                use std::time::Instant;
                let start = Instant::now();
                for _ in 0..iters {
                    rt.block_on(async {
                        let resp = client
                            .post(analyze_url)
                            .header("Authorization", "Bearer test")
                            .json(&body)
                            .send()
                            .await
                            .unwrap();
                        black_box(resp.status());
                    });
                }
                start.elapsed()
            })
        });
    };

    run_case("external_allow", &allow_url);
    run_case("external_block", &block_url);
    run_case("external_slow", &slow_url);
    run_case("external_timeout_fail_open", &timeout_url);
    run_case("external_timeout_fail_closed", &timeout_fail_closed_url);
    run_case("external_chain_three_allow", &multi_chain_url);
}

criterion_group!(external_http_group, bench_scenarios);
criterion_main!(external_http_group);
