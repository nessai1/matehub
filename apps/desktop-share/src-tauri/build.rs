fn main() {
    // app_manifest: автогенерация ACL-пермишенов `allow-<команда>` для
    // app-команд — без этого их нельзя выдать remote-origin'у хаба
    // (runtime capability в connect_hub ссылается на эти идентификаторы).
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "capture_support",
            "request_capture_permission",
            "list_targets",
            "start_share",
            "stop_share",
            "list_hubs",
            "connect_hub",
            "leave_hub",
            "join_voice",
            "leave_voice",
            "set_mute",
            "set_deafen",
        ]),
    ))
    .expect("failed to run tauri-build");
}
