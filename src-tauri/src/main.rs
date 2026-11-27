fn main() {
    // A worker must start nothing else: no Tauri, plugins, logging, or database.
    threestrands_lib::run_pdf_worker_if_requested();
    threestrands_lib::run_transcription_worker_if_requested();
    threestrands_lib::run();
}
