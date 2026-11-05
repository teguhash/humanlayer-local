mod agent;
mod shell_env;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(agent::Sessions::default())
        .setup(|_app| {
            // Warm the shell-env cache off the main thread so the first
            // Send is fast. Errors surface later, on first use.
            std::thread::spawn(shell_env::get);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            agent::start_session,
            agent::respond_approval,
            agent::stop_session
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
