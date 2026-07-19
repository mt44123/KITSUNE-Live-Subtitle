import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";

// Phase 2: Rust コマンドを呼び出す。
// list_audio_devices で出力デバイス名を表示し、start_audio_capture で
// ループバック取得を開始する。音声処理そのものは Rust 側に置き、
// ここでは呼び出しと表示状態の管理だけを行う（UI は表示のみ）。

const INPUT_LANGUAGES = ["Auto Detect", "English", "Japanese", "Korean"];
const TARGET_LANGUAGES = ["Japanese", "English", "Korean"];

function App() {
  const [isCapturing, setIsCapturing] = useState(false);
  const [inputLanguage, setInputLanguage] = useState("Auto Detect");
  const [targetLanguage, setTargetLanguage] = useState("Japanese");

  const [captureError, setCaptureError] = useState<string | null>(null);

  const [outputDevices, setOutputDevices] = useState<string[]>([]);
  const [isCheckingDevices, setIsCheckingDevices] = useState(false);
  const [deviceError, setDeviceError] = useState<string | null>(null);

  const startCapture = async () => {
    setCaptureError(null);
    try {
      await invoke("start_audio_capture");
      setIsCapturing(true);
    } catch (error) {
      setCaptureError(
        typeof error === "string" ? error : "音声キャプチャの開始に失敗しました。"
      );
    }
  };

  const checkAudioDevices = async () => {
    setIsCheckingDevices(true);
    setDeviceError(null);
    try {
      const devices = await invoke<string[]>("list_audio_devices");
      setOutputDevices(devices);
    } catch (error) {
      setOutputDevices([]);
      setDeviceError(
        typeof error === "string" ? error : "デバイスの取得に失敗しました。"
      );
    } finally {
      setIsCheckingDevices(false);
    }
  };

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
          onClick={startCapture}
          disabled={isCapturing}
        >
          {isCapturing ? "Capturing..." : "Start Capture"}
        </button>

        {captureError && <p className="device-error">{captureError}</p>}

        <button
          type="button"
          className="secondary-button"
          onClick={checkAudioDevices}
          disabled={isCheckingDevices}
        >
          {isCheckingDevices ? "Checking..." : "Check Audio Device"}
        </button>

        {deviceError && <p className="device-error">{deviceError}</p>}

        {outputDevices.length > 0 && (
          <div className="device-list">
            <h3 className="device-list-title">Available Output Devices</h3>
            <ul className="device-list-items">
              {outputDevices.map((device) => (
                <li key={device} className="device-list-item">
                  {device}
                </li>
              ))}
            </ul>
          </div>
        )}
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
