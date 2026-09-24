//! The WinRT half: `Windows.Media.SpeechSynthesis.SpeechSynthesizer`.
//!
//! Every WinRT call runs on a thread of this crate's own, which joins the
//! multithreaded apartment and blocks on the async operations — never on
//! GPUI's thread. The caller waits for that thread's answer for at most
//! [`DEADLINE`]; an answer that never comes is a failure, and the thread
//! is left to finish (or not) on its own, since a WinRT call cannot be
//! cancelled from outside.
//!
//! The engine renders into an in-memory stream with
//! `SynthesizeTextToStreamAsync`: nothing ever plays through the speaker.
//! The text is plain text, never SSML.

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use voice_me_core::{AudioBuffer, StockVoice, TtsPort, VoiceMeError};
use windows::Media::SpeechSynthesis::{SpeechSynthesizer, VoiceInformation};
use windows::Storage::Streams::DataReader;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};
use windows::core::HSTRING;

use crate::voices::{RawVoice, default_first, to_stock_voices};
use crate::wav::decode_wav;
use crate::{DEADLINE, LIST_DEADLINE, failure};

/// The voices Windows' speech engine has installed, the default first.
/// No voice at all is a failure: the System voice cannot speak.
pub fn list_voices() -> Result<Vec<StockVoice>, VoiceMeError> {
    let raw = on_worker(LIST_DEADLINE, "listing its voices", all_voices)?;
    let voices = to_stock_voices(raw);
    if voices.is_empty() {
        return Err(failure("lists no installed voices".to_string()));
    }
    Ok(voices)
}

/// How many voices Windows' speech engine has installed — zero included —
/// or, when it cannot be reached at all, why, in words naming Windows
/// speech. What the Dependency Check's System voice row asks (decision 3).
pub fn count_voices() -> Result<usize, String> {
    on_worker(LIST_DEADLINE, "listing its voices", all_voices)
        .map(|raw| to_stock_voices(raw).len())
        .map_err(|error| match error {
            VoiceMeError::SpeechEngine(reason) => reason,
            other => other.to_string(),
        })
}

/// The System voice's [`TtsPort`] adapter on Windows.
#[derive(Debug, Clone)]
pub struct SystemVoiceWindows {
    deadline: Duration,
}

impl Default for SystemVoiceWindows {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemVoiceWindows {
    /// The adapter over Windows' own speech engine.
    pub fn new() -> Self {
        Self { deadline: DEADLINE }
    }
}

impl TtsPort for SystemVoiceWindows {
    /// Nothing to build: each utterance makes its own synthesizer.
    fn warm_up(&self) -> Result<(), VoiceMeError> {
        Ok(())
    }

    fn is_ready(&self) -> bool {
        true
    }

    /// Speak `text` in `voice`, a `VoiceInformation::Id`. A stock voice:
    /// `reference_clip` is never read, and `language` is implied by the
    /// voice.
    fn generate(
        &self,
        text: &str,
        _reference_clip: Option<&Path>,
        _language: &str,
        voice: Option<&str>,
    ) -> Result<AudioBuffer, VoiceMeError> {
        if text.trim().is_empty() {
            return Err(VoiceMeError::EmptyText);
        }
        let Some(voice) = voice else {
            return Err(failure("was given no voice to speak in".to_string()));
        };
        let text = text.to_string();
        let voice = voice.to_string();
        let wav = on_worker(self.deadline, "speaking", move || synthesize(&text, &voice))?;
        decode_wav(&wav).map_err(|reason| failure(format!("failed: {reason}")))
    }
}

/// Run `work` on a new thread in the multithreaded apartment and wait for
/// it for at most `deadline`. `doing` names the work in a failure.
fn on_worker<T, F>(deadline: Duration, doing: &str, work: F) -> Result<T, VoiceMeError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("voice-me-windows-speech".to_string())
        .spawn(move || {
            // SAFETY: called once on this fresh thread, before any other
            // WinRT call on it, and balanced below.
            let result = match unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
                Ok(()) => {
                    let result = work();
                    // SAFETY: balances the successful `RoInitialize` above;
                    // every WinRT object `work` made has been dropped.
                    unsafe { RoUninitialize() };
                    result
                }
                Err(error) => Err(format!("could not start WinRT: {}", reason(&error))),
            };
            // The caller may have given up at the deadline.
            let _ = tx.send(result);
        })
        .map_err(|error| failure(format!("could not start its worker thread: {error}")))?;

    match rx.recv_timeout(deadline) {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(reason)) => Err(failure(format!("failed while {doing}: {reason}"))),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(failure(format!(
            "did not finish {doing} within {} seconds",
            deadline.as_secs()
        ))),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err(failure(format!("stopped unexpectedly while {doing}")))
        }
    }
}

/// A WinRT error in words: its message, or its HRESULT when it has none.
fn reason(error: &windows::core::Error) -> String {
    let message = error.message();
    let message = message.trim();
    if message.is_empty() {
        format!("error {:#010x}", error.code().0)
    } else {
        format!("{message} ({:#010x})", error.code().0)
    }
}

/// `AllVoices`, with the default voice first. Runs on the worker.
fn all_voices() -> Result<Vec<RawVoice>, String> {
    let describe = |voice: &VoiceInformation| -> windows::core::Result<RawVoice> {
        Ok(RawVoice {
            id: voice.Id()?.to_string(),
            display_name: voice.DisplayName()?.to_string(),
            language: voice.Language()?.to_string(),
        })
    };
    let all = SpeechSynthesizer::AllVoices().map_err(|error| reason(&error))?;
    let count = all.Size().map_err(|error| reason(&error))?;
    let mut voices = Vec::with_capacity(count as usize);
    for index in 0..count {
        let voice = all.GetAt(index).map_err(|error| reason(&error))?;
        voices.push(describe(&voice).map_err(|error| reason(&error))?);
    }
    // No default is not a failure: the list order stands.
    let default = SpeechSynthesizer::DefaultVoice()
        .and_then(|voice| voice.Id())
        .ok()
        .map(|id| id.to_string());
    Ok(default_first(voices, default.as_deref()))
}

/// Render `text` in the voice whose id is `voice_id` into WAV bytes. Runs
/// on the worker.
fn synthesize(text: &str, voice_id: &str) -> Result<Vec<u8>, String> {
    let at = |error: windows::core::Error| reason(&error);

    let all = SpeechSynthesizer::AllVoices().map_err(at)?;
    let mut chosen = None;
    for index in 0..all.Size().map_err(at)? {
        let voice = all.GetAt(index).map_err(at)?;
        // Trimmed on both sides, as `to_stock_voices` trims the listed id.
        if voice.Id().map_err(at)?.to_string().trim() == voice_id.trim() {
            chosen = Some(voice);
            break;
        }
    }
    let Some(voice) = chosen else {
        return Err(format!("no installed voice has the id {voice_id:?}"));
    };

    let synthesizer = SpeechSynthesizer::new().map_err(at)?;
    synthesizer.SetVoice(&voice).map_err(at)?;
    let stream = synthesizer
        .SynthesizeTextToStreamAsync(&HSTRING::from(text))
        .map_err(at)?
        .join()
        .map_err(at)?;

    let size = stream.Size().map_err(at)?;
    let input = stream.GetInputStreamAt(0).map_err(at)?;
    let reader = DataReader::CreateDataReader(&input).map_err(at)?;
    let mut bytes = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
    let mut remaining = size;
    while remaining > 0 {
        let want = u32::try_from(remaining).unwrap_or(u32::MAX);
        let loaded = reader.LoadAsync(want).map_err(at)?.join().map_err(at)?;
        if loaded == 0 {
            break;
        }
        let start = bytes.len();
        bytes.resize(start + loaded as usize, 0);
        reader.ReadBytes(&mut bytes[start..]).map_err(at)?;
        remaining = remaining.saturating_sub(u64::from(loaded));
    }
    if bytes.is_empty() {
        return Err("it rendered no audio".to_string());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Empty text row: refused before any WinRT call.
    #[test]
    fn no_text_and_no_voice_are_refused_before_winrt() {
        let adapter = SystemVoiceWindows::new();
        assert!(matches!(
            adapter.generate("", None, "tr-TR", Some("any")),
            Err(VoiceMeError::EmptyText)
        ));
        assert!(matches!(
            adapter.generate("  ", None, "tr-TR", Some("any")),
            Err(VoiceMeError::EmptyText)
        ));
        let error = adapter
            .generate("merhaba", None, "tr-TR", None)
            .unwrap_err();
        assert!(error.to_string().contains("Windows speech"), "{error}");
    }

    #[test]
    fn a_worker_past_its_deadline_is_reported() {
        let error = on_worker(Duration::from_millis(50), "speaking", || {
            std::thread::sleep(Duration::from_secs(2));
            Ok(())
        })
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("Windows speech") && error.contains("did not finish speaking"),
            "{error}"
        );
    }
}
