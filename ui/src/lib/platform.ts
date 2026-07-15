export type DesktopPlatform = "macos" | "windows";

export const desktopPlatform: DesktopPlatform =
  typeof navigator !== "undefined" && /Windows/i.test(navigator.userAgent)
    ? "windows"
    : "macos";

export const isWindows = desktopPlatform === "windows";

export function folderBasename(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? "Untitled";
}

export const openFolderLabel = isWindows ? "Show in File Explorer" : "Reveal in Finder";
export const appShortcutLabel = isWindows ? "Ctrl+O" : "⌘O";
