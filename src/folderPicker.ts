// The folder picker is the only plugin permission this app needs.
// No network plugin, no shell, no filesystem write permissions are requested:
// every read and write goes through novelvault-core, which confines writes to
// .novelvault/ and opens author files read-only.
export async function chooseNovelFolder(): Promise<string | null> {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const selected = await open({
    directory: true,
    multiple: false,
    title: "Select the novel folder",
  });
  return typeof selected === "string" ? selected : null;
}
