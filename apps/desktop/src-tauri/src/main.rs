#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    use aeye_desktop::host::{Action, Host, Outcome};
    use aeye_desktop::worker::Worker;
    use tauri::Manager;

    #[tauri::command]
    fn desktop_action(
        state: tauri::State<'_, Worker>,
        action: Action,
        reply: tauri::ipc::Channel<Outcome>,
    ) -> Result<(), String> {
        // Enqueue synchronously in IPC arrival order, then return without waiting on native work.
        state.submit(action, move |outcome| {
            // A closed window can discard its reply; the native action still finishes safely.
            let _ = reply.send(outcome);
        })
    }

    let executable = std::env::current_exe().expect("locate the desktop executable");
    let sidecar = executable
        .parent()
        .expect("desktop executable directory")
        .join("aeye-mcp.exe");
    tauri::Builder::default()
        .manage(Worker::start(Host::new(sidecar)))
        .invoke_handler(tauri::generate_handler![desktop_action])
        .build(tauri::generate_context!())
        .expect("build the A-Eye desktop window")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                app.state::<Worker>().shutdown();
            }
        });
}

#[cfg(not(windows))]
fn main() {
    eprintln!(
        "The desktop shell currently requires native Windows; portable host tests are available."
    );
    std::process::exit(2);
}
