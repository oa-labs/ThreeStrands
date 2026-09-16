fn main() {
    println!("cargo:rerun-if-env-changed=DISPATCH_GOOGLE_CLIENT_ID");
    println!("cargo:rerun-if-env-changed=DISPATCH_GOOGLE_CLIENT_SECRET");

    if std::env::var("PROFILE").as_deref() == Ok("release") {
        for name in ["DISPATCH_GOOGLE_CLIENT_ID", "DISPATCH_GOOGLE_CLIENT_SECRET"] {
            if std::env::var(name)
                .ok()
                .is_none_or(|value| value.trim().is_empty())
            {
                panic!("{name} is required for a release build");
            }
        }
    }

    tauri_build::build()
}
