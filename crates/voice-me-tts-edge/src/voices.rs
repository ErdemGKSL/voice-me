//! `edge-tts --list-voices`, parsed.

use voice_me_core::StockVoice;

/// Every voice in `edge-tts --list-voices` output.
///
/// The output is a fixed-width table: a header (`Name  Gender  …`), a rule
/// of dashes, then one voice per row. The first field of a row is the voice
/// id (`tr-TR-EmelNeural`) and the second its gender. The header, the rule,
/// blank rows and rows without that shape are skipped.
///
/// - The locale is the id without its last `-` segment: `tr-TR`,
///   `zh-CN-liaoning`, `iu-Cans-CA`. It is also the language label, since
///   the program names no languages.
/// - The name is that last segment without `MultilingualNeural` or
///   `Neural`, then the gender: "Emel (Female)".
/// - The priority is the list order, so a language's first listed voice is
///   its default.
pub fn parse_voices(text: &str) -> Vec<StockVoice> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let id = fields.next()?;
            let gender = fields.next()?;
            if id == "Name" || id.starts_with('-') {
                return None;
            }
            let (locale, voice) = id.rsplit_once('-')?;
            if locale.is_empty() || voice.is_empty() {
                return None;
            }
            let short = voice
                .strip_suffix("MultilingualNeural")
                .or_else(|| voice.strip_suffix("Neural"))
                .filter(|short| !short.is_empty())
                .unwrap_or(voice);
            Some((id, locale, format!("{short} ({gender})")))
        })
        .enumerate()
        .map(|(index, (id, locale, name))| StockVoice {
            id: id.to_string(),
            language: locale.to_string(),
            language_label: locale.to_string(),
            name,
            priority: u32::try_from(index).unwrap_or(u32::MAX),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use voice_me_core::stock_voices_of;

    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/list-voices-7.2.8.txt");

    fn voice<'a>(voices: &'a [StockVoice], id: &str) -> &'a StockVoice {
        voices
            .iter()
            .find(|voice| voice.id == id)
            .unwrap_or_else(|| panic!("{id} is listed"))
    }

    #[test]
    fn the_fixture_parses_without_its_header_or_rule() {
        let voices = parse_voices(FIXTURE);

        // 324 lines: the header, the rule, and 322 voices.
        let rows = FIXTURE
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count();
        assert_eq!(voices.len(), rows - 2);
        assert!(
            voices
                .iter()
                .all(|voice| voice.id != "Name" && !voice.id.starts_with('-'))
        );
        assert_eq!(voices[0].id, "af-ZA-AdriNeural");
        assert_eq!(voices[0].priority, 0);
    }

    #[test]
    fn a_voice_has_its_locale_name_and_gender() {
        let voices = parse_voices(FIXTURE);

        let emel = voice(&voices, "tr-TR-EmelNeural");
        assert_eq!(emel.language, "tr-TR");
        assert_eq!(emel.language_label, "tr-TR");
        assert_eq!(emel.name, "Emel (Female)");
        assert_eq!(voice(&voices, "tr-TR-AhmetNeural").name, "Ahmet (Male)");

        let ava = voice(&voices, "en-US-AvaMultilingualNeural");
        assert_eq!(ava.language, "en-US");
        assert_eq!(ava.name, "Ava (Female)");
    }

    #[test]
    fn three_and_four_segment_locales_keep_every_segment_but_the_last() {
        let voices = parse_voices(FIXTURE);

        let liaoning = voices
            .iter()
            .find(|voice| voice.id.starts_with("zh-CN-liaoning-"))
            .expect("a zh-CN-liaoning voice is listed");
        assert_eq!(liaoning.language, "zh-CN-liaoning");

        let inuktitut = voices
            .iter()
            .find(|voice| voice.id.starts_with("iu-Cans-CA-"))
            .expect("an iu-Cans-CA voice is listed");
        assert_eq!(inuktitut.language, "iu-Cans-CA");
    }

    /// Turkish's default voice is its first listed one: Ahmet.
    #[test]
    fn the_list_order_is_the_priority() {
        let voices = parse_voices(FIXTURE);
        let turkish = stock_voices_of(&voices, "tr-TR");
        assert_eq!(turkish[0].id, "tr-TR-AhmetNeural");
        assert_eq!(turkish[1].id, "tr-TR-EmelNeural");
    }

    #[test]
    fn blank_malformed_and_header_only_output_yields_no_voices() {
        assert!(parse_voices("").is_empty());
        assert!(
            parse_voices(
                "Name   Gender  ContentCategories  VoicePersonalities\n\
                 -----  ------  -----------------  ------------------\n\
                 \n\
                 lonely\n\
                 nodash Female\n"
            )
            .is_empty()
        );
    }
}
