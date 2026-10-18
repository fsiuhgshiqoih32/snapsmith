# Changelog

## 0.1.2

- Fixed the screen going black for a moment when the selection opened or closed (screenshots,
  GIF and MP4 recording) on Windows with an HDR or variable-refresh monitor. With OpenGL, the
  graphics driver showed the full-screen selection directly instead of through Windows'
  compositor, which made such monitors switch modes. Full-screen windows now use DX12 on
  Windows; small windows keep OpenGL. Shortcuts still show the selection in under a tenth of a
  second; one started from scratch takes about half a second (a third with OpenGL).
- GIF recordings are written to disk frame by frame, so one cut short (the computer shut down
  mid-recording) still plays up to that point.

## 0.1.1

- Much faster start. On Windows the selection now appears about a tenth of a second after a
  shortcut (about a third of a second when started from scratch), instead of one to two
  seconds. macOS and Linux get the same changes except the waiting window; they were not
  timed.
  - Windows open with OpenGL, which starts several times faster than DX12/Vulkan here; wgpu is
    still used automatically where OpenGL isn't available.
  - The screen is captured while the window opens, and ffmpeg is found without starting it.
  - On Windows a small hidden window waits ready, so shortcuts and the main window's buttons
    don't start from scratch (`SNAPSMITH_STANDBY=0` turns this off).
  - The main window hides at once instead of minimising first.
- Errors from shortcuts and the tray (MP4 recording without ffmpeg, say) now show a
  notification instead of failing silently.

## 0.1.0

First release, for Windows (x64 and ARM64), macOS (Apple silicon and Intel) and Linux (x86_64).

- Capture a region on a frozen screen (with window snapping, magnifier and colour picker), the
  focused window, the monitor under the pointer, or all screens.
- Annotate in place: rectangle, ellipse, arrow, line, pen, highlighter, text, numbered steps,
  pixelate and blur, with undo and redo.
- After-capture tasks: copy, save with file name patterns, upload and copy the link, recognise
  text, pin to screen, notify.
- Uploads to Catbox and Litterbox, plus ShareX custom uploaders (`.sxcu`, current and legacy
  formats).
- On-device text recognition with ocrs; models are downloaded once on request.
- GIF recording, and MP4 recording through ffmpeg.
- Image editor with crop, history window with thumbnails, tray icon, global shortcuts, start at
  login, update check, and a command line for every action.
