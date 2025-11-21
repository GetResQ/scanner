use std::time::Duration;

use anyhow::Result;

use crate::pool::Pool;
use crate::ui;

pub async fn run_demo(use_tui: bool) -> Result<()> {
    // Create a demo pool
    let pool = Pool::new(4);
    let (ui_tx, ui_handle) = ui::spawn_ui(use_tui, true, false, pool.clone());

    let checks = [
        ("rust-lint", "Run clippy on workspace"),
        ("rust-test", "Run unit tests"),
        ("fmt", "Check formatting"),
        ("js-lint", "Frontend lint"),
        ("types", "Type checking"),
        ("security", "Security scan"),
    ];

    let mut handles = Vec::new();
    for (idx, (name, desc)) in checks.iter().enumerate() {
        let name = name.to_string();
        let desc = desc.to_string();
        let tx = ui_tx.clone();
        let sleep_ms = 600 + (idx as u64 * 450);
        let fail = idx % 4 == 3; // make some failures

        // Use the pool to schedule tasks
        handles.push(pool.spawn(async move {
            if let Some(tx) = tx.as_ref() {
                let _ = tx
                    .send(ui::UiEvent::CheckStarted {
                        name: name.clone(),
                        desc: Some(desc.clone()),
                    })
                    .await;
            }
            tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
            if let Some(tx) = tx.as_ref() {
                let _ = tx
                    .send(ui::UiEvent::CheckFinished {
                        name: name.clone(),
                        success: !fail,
                        message: if fail {
                            "simulated failure".to_string()
                        } else {
                            "ok".to_string()
                        },
                        output: Some(format!("log output for {name} (simulated)")),
                    })
                    .await;
            }
        }));
    }

    for handle in handles {
        let _ = handle.await;
    }

    if let Some(tx) = ui_tx {
        let _ = tx.send(ui::UiEvent::Done).await;
    }
    let _ = ui_handle.await;
    Ok(())
}
