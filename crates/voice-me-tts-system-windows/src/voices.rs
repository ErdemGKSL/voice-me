//! `SpeechSynthesizer::AllVoices`, mapped to stock voices. Pure, so it is
//! tested on every OS; the WinRT calls that fill [`RawVoice`] live in
//! `engine.rs`.

use voice_me_core::StockVoice;

/// One voice as WinRT's `VoiceInformation` describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawVoice {
    /// `VoiceInformation::Id`: what `SetVoice` is matched against.
    pub id: String,
    /// `VoiceInformation::DisplayName` ("Microsoft Tolga").
    pub display_name: String,
    /// `VoiceInformation::Language`, a BCP-47 tag ("tr-TR").
    pub language: String,
}

/// `voices` with the one whose id is `default_id` moved to the front, the
/// rest in list order — so the engine's default voice is its language's
/// top-priority one.
pub fn default_first(mut voices: Vec<RawVoice>, default_id: Option<&str>) -> Vec<RawVoice> {
    if let Some(default_id) = default_id
        && let Some(at) = voices.iter().position(|voice| voice.id == default_id)
    {
        let default = voices.remove(at);
        voices.insert(0, default);
    }
    voices
}

/// The stock voices `voices` describe. The priority is the list order, so
/// with [`default_first`] the default voice comes first. The language is
/// the voice's BCP-47 tag as reported, and both the name and the language
/// label are its display name. A voice with no id or no language, or an
/// id already listed, is skipped.
pub fn to_stock_voices(voices: Vec<RawVoice>) -> Vec<StockVoice> {
    let mut out: Vec<StockVoice> = Vec::with_capacity(voices.len());
    for voice in voices {
        let id = voice.id.trim();
        let language = voice.language.trim();
        if id.is_empty() || language.is_empty() || out.iter().any(|seen| seen.id == id) {
            continue;
        }
        let name = match voice.display_name.trim() {
            "" => id.to_string(),
            name => name.to_string(),
        };
        out.push(StockVoice {
            id: id.to_string(),
            language: language.to_string(),
            language_label: name.clone(),
            name,
            priority: out.len() as u32,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use voice_me_core::{LanguageBackend, resolve_stock_voice, stock_voice_languages};

    use super::*;

    const TOLGA: &str =
        r"HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Speech_OneCore\Voices\Tokens\MSTTS_V110_trTR_Tolga";
    const DAVID: &str = r"HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Speech_OneCore\Voices\Tokens\MSTTS_V110_enUS_DavidM";
    const ZIRA: &str =
        r"HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Speech_OneCore\Voices\Tokens\MSTTS_V110_enUS_ZiraM";

    fn raw(id: &str, display_name: &str, language: &str) -> RawVoice {
        RawVoice {
            id: id.to_string(),
            display_name: display_name.to_string(),
            language: language.to_string(),
        }
    }

    fn installed() -> Vec<RawVoice> {
        vec![
            raw(TOLGA, "Microsoft Tolga", "tr-TR"),
            raw(DAVID, "Microsoft David", "en-US"),
            raw(ZIRA, "Microsoft Zira", "en-US"),
        ]
    }

    /// The Voice list row.
    #[test]
    fn tolga_david_and_zira_map_to_stock_voices_in_list_order() {
        let voices = to_stock_voices(installed());

        assert_eq!(voices.len(), 3);
        assert_eq!(
            voices[0],
            StockVoice {
                id: TOLGA.to_string(),
                language: "tr-TR".to_string(),
                language_label: "Microsoft Tolga".to_string(),
                name: "Microsoft Tolga".to_string(),
                priority: 0,
            }
        );
        assert_eq!(voices[1].language, "en-US");
        assert_eq!(voices[1].name, "Microsoft David");
        assert_eq!(voices[2].priority, 2);

        let languages: Vec<String> = stock_voice_languages(&voices)
            .into_iter()
            .map(|language| language.code)
            .collect();
        assert_eq!(languages, ["en-US", "tr-TR"]);
    }

    #[test]
    fn the_default_voice_comes_first_and_is_its_languages_default() {
        let voices = to_stock_voices(default_first(installed(), Some(ZIRA)));
        assert_eq!(voices[0].id, ZIRA);
        assert_eq!(voices[0].priority, 0);
        assert_eq!(voices[1].id, TOLGA);
        assert_eq!(voices[2].id, DAVID);

        let english =
            resolve_stock_voice(LanguageBackend::SystemVoice, &voices, "en-US", None).unwrap();
        assert_eq!(english.id, ZIRA);

        // An unknown or missing default leaves the order alone.
        assert_eq!(default_first(installed(), Some("nope")), installed());
        assert_eq!(default_first(installed(), None), installed());
    }

    /// Decision 1: the saved default `tr` finds Tolga's `tr-TR`.
    #[test]
    fn the_saved_tr_speaks_through_tolga() {
        let voices = to_stock_voices(installed());
        let voice = resolve_stock_voice(LanguageBackend::SystemVoice, &voices, "tr", None).unwrap();
        assert_eq!(voice.id, TOLGA);
    }

    #[test]
    fn blank_and_duplicate_voices_are_skipped() {
        let voices = to_stock_voices(vec![
            raw("", "Nameless", "en-US"),
            raw("a", "No language", "  "),
            raw("b", "  ", "de-DE"),
            raw("b", "Again", "de-DE"),
        ]);
        assert_eq!(voices.len(), 1);
        assert_eq!(voices[0].id, "b");
        assert_eq!(voices[0].name, "b");
        assert_eq!(voices[0].priority, 0);
    }
}
