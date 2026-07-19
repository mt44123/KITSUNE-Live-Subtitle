//! 翻訳レイヤー。
//!
//! Whisper が出力した認識テキストを翻訳してから字幕として届けるための、独立した責務の
//! モジュール。現時点では実際の翻訳は行わず、入力テキストをそのまま返す「ダミー翻訳」
//! だけを提供する。
//!
//! # 責務
//!
//! - 入力文字列を受け取り、翻訳後の文字列を返すことだけを担当する。
//! - Whisper・音声パイプライン・Rolling Window・差分抽出・React・Tauri のいずれの
//!   内部構造にも依存しない。純粋に「テキスト → テキスト」の変換のみを行う。
//!
//! # 呼び出し側との関係
//!
//! 呼び出し側（Whisper 側）は [`translate`] を呼ぶだけで、翻訳の実装詳細を一切知らない。
//! パイプラインは `Whisper → Translator → React` の順で流れる。
//!
//! # 将来の差し替え
//!
//! 将来 Google / DeepL / OpenAI / Gemini などの翻訳プロバイダへ差し替える際は、この
//! モジュール内の実装だけを変更（または実装を選択）すればよい。呼び出し側は `translate`
//! を呼び続けるだけで変更不要なため、翻訳エンジンの入れ替えが局所化される。

/// 入力テキストを翻訳して返す。
///
/// 現時点はダミー翻訳のため、翻訳は行わず入力をそのまま返す。
///
/// # 例
///
/// ```ignore
/// assert_eq!(translate("Hello world"), "Hello world");
/// ```
pub fn translate(text: &str) -> String {
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::translate;

    /// ダミー翻訳は入力文字列をそのまま返す。
    #[test]
    fn translate_returns_input_unchanged() {
        assert_eq!(translate("Hello world"), "Hello world");
    }

    /// 空文字も変換せずそのまま返す。
    #[test]
    fn translate_preserves_empty_string() {
        assert_eq!(translate(""), "");
    }

    /// 日本語・記号などの非 ASCII もそのまま返す。
    #[test]
    fn translate_preserves_non_ascii() {
        assert_eq!(
            translate("こんにちは #1 \"$500\""),
            "こんにちは #1 \"$500\""
        );
    }
}
