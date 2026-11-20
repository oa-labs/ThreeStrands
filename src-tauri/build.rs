fn main() {
    println!("cargo:rerun-if-env-changed=THREESTRANDS_GOOGLE_CLIENT_ID");
    println!("cargo:rerun-if-env-changed=THREESTRANDS_GOOGLE_CLIENT_SECRET");

    if std::env::var("PROFILE").as_deref() == Ok("release")
        && !matches!(
            std::env::var("CARGO_CFG_TARGET_OS").as_deref(),
            Ok("android" | "ios")
        )
    {
        for name in [
            "THREESTRANDS_GOOGLE_CLIENT_ID",
            "THREESTRANDS_GOOGLE_CLIENT_SECRET",
        ] {
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
