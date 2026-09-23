//! `espeak-ng --voices`, parsed.

use voice_me_core::SystemVoice;

/// Every voice in `espeak-ng --voices` output.
///
/// Each line reads `Pty Language Age/Gender VoiceName File [Other…]`. The
/// header, `variant` lines and anything that does not have that shape are
/// skipped. The id is `File` (`gmw/en-US`), which is unique and is what
/// `-v` takes; the name is `VoiceName` with `_` read as a space.
pub fn parse_voices(text: &str) -> Vec<SystemVoice> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let priority = fields.next()?.parse::<u32>().ok()?;
            let language = fields.next()?;
            let _age_gender = fields.next()?;
            let name = fields.next()?;
            let file = fields.next()?;
            if language == "variant" {
                return None;
            }
            Some(SystemVoice {
                id: file.to_string(),
                language: language.to_string(),
                name: name.replace('_', " ").trim().to_string(),
                priority,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from eSpeak NG 1.52 (`espeak-ng --voices`), trimmed.
    const FIXTURE: &str = "\
Pty Language       Age/Gender VoiceName          File                 Other Languages
 5  af              --/M      Afrikaans          gmw/af               
 5  chr-US-Qaaa-x-west --/M      Cherokee_          iro/chr              
 5  cmn             --/M      Chinese_(Mandarin,_latin_as_English) sit/cmn              (zh-cmn 5)(zh 5)
 2  en-gb           --/M      English_(Great_Britain) gmw/en               (en 2)
 5  en-us           --/M      English_(America)  gmw/en-US            (en 3)
 5  tr              --/M      Turkish            trk/tr               
 5  yue             --/M      Chinese_(Cantonese) sit/yue              (zh-yue 5)(zh 8)
 5  yue             --/M      Chinese_(Cantonese,_latin_as_Jyutping) sit/yue-Latn-jyutping (zh-yue 5)(zh 8)
 5  variant         --/M      Adam               !v/adam              
";

    #[test]
    fn a_captured_voice_list_parses_with_variants_and_the_header_skipped() {
        let voices = parse_voices(FIXTURE);

        assert_eq!(voices.len(), 8, "{voices:#?}");
        assert!(voices.iter().all(|voice| voice.language != "variant"));
        assert!(voices.iter().all(|voice| voice.language != "Language"));

        let turkish = voices.iter().find(|voice| voice.id == "trk/tr").unwrap();
        assert_eq!(turkish.language, "tr");
        assert_eq!(turkish.name, "Turkish");
        assert_eq!(turkish.priority, 5);

        let british = voices.iter().find(|voice| voice.id == "gmw/en").unwrap();
        assert_eq!(british.name, "English (Great Britain)");
        assert_eq!(british.priority, 2);

        let jyutping = voices.last().unwrap();
        assert_eq!(jyutping.id, "sit/yue-Latn-jyutping");
        assert_eq!(jyutping.name, "Chinese (Cantonese, latin as Jyutping)");

        // A trailing underscore is not a trailing space.
        let cherokee = voices.iter().find(|voice| voice.id == "iro/chr").unwrap();
        assert_eq!(cherokee.name, "Cherokee");
        assert_eq!(cherokee.language, "chr-US-Qaaa-x-west");
    }

    #[test]
    fn garbage_parses_to_nothing() {
        assert!(parse_voices("").is_empty());
        assert!(parse_voices("not a voice list\n\x07\n5 tr").is_empty());
    }
}
