# ThreeStrands for Android

ThreeStrands for Android opens to today's Daily Note. Use **Capture** to
write into the daily note or create a standalone note or task. In a note,
**Checklist** inserts a square checkbox and **Task** inserts a round one.
Tasks from notes also appear in **Tasks**. Tap one there to open its note.

Notes and tasks can be saved and searched offline. ThreeStrands keeps
drafts on the phone. If a saved note changes while you edit, compare the
versions before you decide which text to keep.

## Pair a phone with desktop

1. On desktop, open **Settings → Integrations → Replicated Sync** and turn on
   **Enable replicated sync**. Choose **S3 storage** and enter the HTTPS
   endpoint, region, bucket, and access key credentials from your provider.
   Choose **Test Connection**, then **Add connector**. If this is your first
   device, choose **Create a new sync group** and save the recovery phrase.
   [Replicated Sync and S3 storage](cross-device-sync.md#s3-storage)
2. In **Devices → Add a device**, choose only the S3 connector for the join
   code. Decide whether to include its credentials.
3. On Android, open **Sync settings**, paste the code, and tap
   **Review Join Code**. Check the connector and enter credentials if the
   code omitted them. Tap **Join Sync Group**.
4. Sync once more on desktop to admit the phone. On Android, reopen the
   app or choose **Sync Now**. On desktop, use the **All accounts** scope in
   **Tasks** to see standalone tasks created on the phone.

The phone joins the entire encrypted group. Notes and Tasks are the phone's
workspaces. The application also preserves the rest of the group's records
when it syncs. Android shows existing attachment references in note text.
Use desktop to open their files.

**Export Notes and Tasks** in **Sync settings** creates a ZIP with Markdown
notes and a copy of the task records. Use it to save a copy of work stored
on the phone before clearing the app's data. Export files contain readable
text. Save them somewhere you control. **Disconnect and Keep Data** stops
sync on the phone and keeps its local notes and tasks.

## Build and test

Install Android SDK 36, build tools 36.0.0, NDK 28.2.13676358, and JDK 21.
Set `ANDROID_HOME` to the SDK and `NDK_HOME` to the NDK. Run from the repo
root after `pnpm install --frozen-lockfile`.

```sh
rustup target add aarch64-linux-android
pnpm exec vitest run apps/mobile
pnpm tauri android build --apk --debug --target aarch64 --ci
```

The tracked Android project under `src-tauri/gen/android` supplies the
launcher, secure store, and Android build settings. The mobile entry point
and capability expose Notes, Tasks, and sync. The Rust build uses the
existing database and sync implementation. The UI builds from
`apps/mobile`. The Android build uses SDK 28 as its minimum and an ARM64
APK in CI.

Use the generated debug APK with an ARM64 test device and a current Android
System WebView. For the native secure-store test, build the APK first, then
run `./gradlew connectedUniversalDebugAndroidTest` from
`src-tauri/gen/android` with the device connected. Verify these release
scenarios on installed builds.

- In airplane mode, write in a Daily Note, capture a round task and a square
  checklist, and confirm that the task appears once in **Tasks**. Stop the
  app during an edit and verify draft recovery on relaunch. Try multiline
  composition, Android Back, TalkBack, and large text on the phone.
- Pair with a test desktop group over HTTPS S3. Edit and complete note and
  standalone tasks in both directions. Verify offline catch-up, a conflicting
  note edit, attachment references, key rotation, and revocation. Use
  synthetic test content and a disposable connector.
- Install an upgrade over both a local-only and a paired app. Verify draft
  recovery, local search, the same device identity, and continued sync.

## Build a Play test bundle

Run the **Android** workflow manually with an increasing `version_code` to
build a signed AAB. Configure the upload key in `THREESTRANDS_ANDROID_KEYSTORE_BASE64`,
`THREESTRANDS_ANDROID_STORE_PASSWORD`, `THREESTRANDS_ANDROID_KEY_ALIAS`, and
`THREESTRANDS_ANDROID_KEY_PASSWORD`. Download the artifact, review the
current Play declarations for the app, and upload it to a test track.
[Android code signing](https://v2.tauri.app/distribute/sign/android/)
· [Google Play distribution](https://v2.tauri.app/distribute/google-play/)
