import { useState } from "react";
import "./App.css";

// Phase 1: 初期UIのみ。
// 音声取得やTauri(Rust)処理はまだ実装しません。
// ここでの useState は「ボタン表示の切り替え」という画面状態のためだけに使い、
// 実際の音声処理などのビジネスロジックは含めません。

const INPUT_LANGUAGES = ["Auto Detect", "English", "Japanese", "Korean"];
const TARGET_LANGUAGES = ["Japanese", "English", "Korean"];

function App() {
  const [isCapturing, setIsCapturing] = useState(false);
  const [inputLanguage, setInputLanguage] = useState("Auto Detect");
  const [targetLanguage, setTargetLanguage] = useState("Japanese");

  return (
    <main className="app">
      <header className="app-header">
        <h1 className="app-title">KITSUNE Live Subtitle</h1>
        <span className="app-status">Ready</span>
      </header>

      <section className="panel">
        <h2 className="panel-title">Audio Source</h2>
        <div className="field-row">
          <span className="field-label">Source</span>
          <span className="field-value">System Audio</span>
        </div>
        <button
          type="button"
          className="capture-button"
          onClick={() => setIsCapturing((capturing) => !capturing)}
        >
          {isCapturing ? "Stop Capture" : "Start Capture"}
        </button>
      </section>

      <section className="panel">
        <h2 className="panel-title">Language</h2>

        <label className="field-row" htmlFor="input-language">
          <span className="field-label">Input</span>
          <select
            id="input-language"
            className="select"
            value={inputLanguage}
            onChange={(event) => setInputLanguage(event.target.value)}
          >
            {INPUT_LANGUAGES.map((language) => (
              <option key={language} value={language}>
                {language}
              </option>
            ))}
          </select>
        </label>

        <label className="field-row" htmlFor="target-language">
          <span className="field-label">Translate to</span>
          <select
            id="target-language"
            className="select"
            value={targetLanguage}
            onChange={(event) => setTargetLanguage(event.target.value)}
          >
            {TARGET_LANGUAGES.map((language) => (
              <option key={language} value={language}>
                {language}
              </option>
            ))}
          </select>
        </label>
      </section>

      <section className="panel subtitle-preview">
        <h2 className="panel-title">Subtitle Preview</h2>
        <p className="subtitle-original">Let's go!</p>
        <p className="subtitle-translated">行くぞ！</p>
      </section>
    </main>
  );
}

export default App;
