//! Snapsmith's platform-independent parts: settings, file naming, history, uploaders, the
//! annotation renderer, GIF encoding and OCR. The binary adds windows, capture and the tray.

pub mod config;
pub mod editor;
pub mod gifenc;
pub mod history;
pub mod naming;
pub mod ocr;
pub mod upload;
