use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SizedSample, StreamConfig};
use std::sync::{mpsc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// 出力オーディオデバイスを列挙し、既定の出力デバイス名をターミナルに表示する。
///
/// Phase 2 の Step 1/2 に相当する「デバイス確認」だけを行う。
/// ここでは音声のキャプチャ（build_input_stream 等）は一切行わない。
/// 戻り値は「利用可能な出力デバイス名の一覧」で、失敗時は初心者にも分かる
/// エラーメッセージを文字列で返す。
#[tauri::command]
fn list_audio_devices() -> Result<Vec<String>, String> {
    // 既定のホスト（Windows では WASAPI）を取得する。
    let host = cpal::default_host();

    // 利用可能な出力デバイスを列挙する。
    let output_devices = host.output_devices().map_err(|error| {
        format!(
            "出力デバイスの一覧を取得できませんでした。オーディオドライバが有効か確認してください。詳細: {error}"
        )
    })?;

    let mut device_names: Vec<String> = Vec::new();
    for device in output_devices {
        // デバイス名の取得に失敗しても全体を止めず、その1台だけ代替表示にする。
        // `description()` は人が読めるデバイス名（例: "Speakers (Realtek Audio)"）を含む。
        let name = match device.description() {
            Ok(description) => description.name().to_string(),
            Err(error) => format!("(名前を取得できないデバイス: {error})"),
        };
        device_names.push(name);
    }

    // 既定の出力デバイス名を取得する。存在しない場合はエラーにする。
    let default_output_name = match host.default_output_device() {
        Some(device) => device
            .description()
            .map(|description| description.name().to_string())
            .map_err(|error| {
                format!("既定の出力デバイス名を取得できませんでした。詳細: {error}")
            })?,
        None => {
            return Err(
                "既定の出力デバイスが見つかりませんでした。スピーカーやヘッドホンが接続・有効になっているか確認してください。"
                    .to_string(),
            )
        }
    };

    // Rust 側のターミナルへ結果を表示する。
    println!("Default output device: {default_output_name}");
    println!("Available output devices ({}):", device_names.len());
    for name in &device_names {
        println!("  - {name}");
    }

    Ok(device_names)
}

/// ループバック用の入力ストリームを構築する。
///
/// コールバックでは受け取ったサンプル数を数え、あわせて音量(RMS)を集計し、
/// 約1秒ごとに `Received samples: N | RMS: x.xxxxxx` の形式で表示する。
/// （毎コールバックで表示すると大量に出力されるため、1秒間隔にまとめている。）
///
/// サンプル形式(f32 / i16 / u16)ごとに同じ処理を使い回すためジェネリックにし、
/// 各形式を概ね -1.0〜1.0 に正規化する関数 `normalize` を引数で受け取る。
fn build_counting_input_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    normalize: fn(T) -> f64,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: SizedSample + 'static,
{
    let mut sample_count: usize = 0;
    let mut sum_of_squares: f64 = 0.0;
    let mut last_report = Instant::now();

    device.build_input_stream::<T, _, _>(
        config,
        move |data: &[T], _info| {
            sample_count += data.len();
            for &sample in data {
                let value = normalize(sample);
                sum_of_squares += value * value;
            }

            if last_report.elapsed() >= Duration::from_secs(1) {
                // RMS = sqrt(二乗和 / サンプル数)。サンプルが無い場合は 0 とする。
                let rms = if sample_count > 0 {
                    (sum_of_squares / sample_count as f64).sqrt()
                } else {
                    0.0
                };
                println!("Received samples: {sample_count} | RMS: {rms:.6}");

                sample_count = 0;
                sum_of_squares = 0.0;
                last_report = Instant::now();
            }
        },
        move |error| eprintln!("音声ストリームでエラーが発生しました: {error}"),
        None,
    )
}

/// 既定の出力デバイスを WASAPI ループバックとして開き、再生を開始した Stream を返す。
///
/// cpal では「出力デバイスに対して build_input_stream を呼ぶ」とループバックになる。
/// cpal の Stream はスレッドをまたいで送れないため、この関数は必ずキャプチャ用の
/// 専用スレッド内で呼び出し、生成した Stream もそのスレッドで保持する。
fn open_loopback_stream() -> Result<cpal::Stream, String> {
    let host = cpal::default_host();

    let device = host.default_output_device().ok_or_else(|| {
        "既定の出力デバイスが見つかりませんでした。スピーカーやヘッドホンが接続・有効になっているか確認してください。"
            .to_string()
    })?;

    // ループバックでは出力デバイスの再生フォーマットをそのまま使う。
    // （出力デバイスに対する入力用フォーマット取得は空になるため使わない。）
    let default_config = device.default_output_config().map_err(|error| {
        format!("既定の出力デバイスの設定を取得できませんでした。詳細: {error}")
    })?;

    let sample_format = default_config.sample_format();
    let config: StreamConfig = default_config.config();

    let stream = match sample_format {
        // f32 はそのまま(概ね -1.0〜1.0)。
        SampleFormat::F32 => {
            build_counting_input_stream::<f32>(&device, &config, |sample| sample as f64)
        }
        // i16 は最大値で割って正規化する。
        SampleFormat::I16 => build_counting_input_stream::<i16>(&device, &config, |sample| {
            sample as f64 / i16::MAX as f64
        }),
        // u16 は中央値(32768)を 0 として正規化する。
        SampleFormat::U16 => build_counting_input_stream::<u16>(&device, &config, |sample| {
            (sample as f64 - 32768.0) / 32768.0
        }),
        other => {
            return Err(format!(
                "このデバイスのサンプル形式にはまだ対応していません: {other:?}"
            ))
        }
    }
    .map_err(|error| format!("ループバック用の入力ストリームを作成できませんでした。詳細: {error}"))?;

    stream
        .play()
        .map_err(|error| format!("音声ストリームを開始できませんでした。詳細: {error}"))?;

    Ok(stream)
}

/// 実行中のキャプチャを制御するためのハンドル。
///
/// - `stop_sender`: 専用スレッドへ停止を通知するための送信側。
/// - `thread`: 専用スレッドの JoinHandle。Stop 時に join して Stream の drop 完了を待つ。
struct CaptureHandle {
    stop_sender: mpsc::Sender<()>,
    thread: thread::JoinHandle<()>,
}

/// Tauri State として保持するキャプチャ状態。
///
/// `None` なら未開始、`Some` なら実行中。Mutex で二重 Start を防ぐ。
#[derive(Default)]
struct CaptureState {
    handle: Mutex<Option<CaptureHandle>>,
}

/// ループバックキャプチャを開始する。
///
/// 専用スレッドを起動し、その中で Stream 生成と `play()` を行う。生成の成否は
/// チャンネルで受け取り、`play()` まで成功して初めて `Ok(())` を返す
/// （成功前に React 側を Capturing 状態にしないため）。
/// すでに実行中の場合は新しいスレッドを作らず、分かりやすいエラーを返す。
#[tauri::command]
fn start_audio_capture(state: tauri::State<CaptureState>) -> Result<(), String> {
    let mut handle_guard = state
        .handle
        .lock()
        .map_err(|_| "内部状態のロックに失敗しました。".to_string())?;

    if handle_guard.is_some() {
        return Err("すでに音声キャプチャを実行中です。".to_string());
    }

    let (stop_sender, stop_receiver) = mpsc::channel::<()>();
    let (setup_sender, setup_receiver) = mpsc::channel::<Result<(), String>>();

    let thread = thread::spawn(move || match open_loopback_stream() {
        Ok(stream) => {
            // 生成・再生成功を通知したうえで、停止通知が来るまで待機する。
            let _ = setup_sender.send(Ok(()));
            // recv() は () を受信するか、送信側が drop されると返る。どちらでも停止する。
            let _ = stop_receiver.recv();
            // ここで stream が drop され、キャプチャが停止する。
            drop(stream);
        }
        Err(message) => {
            let _ = setup_sender.send(Err(message));
        }
    });

    // Stream の生成と play() の成功を確認してから成功扱いにする。
    match setup_receiver.recv() {
        Ok(Ok(())) => {
            *handle_guard = Some(CaptureHandle {
                stop_sender,
                thread,
            });
            Ok(())
        }
        Ok(Err(message)) => {
            // 失敗時、専用スレッドはすでに終了しているので join で後始末する。
            let _ = thread.join();
            Err(message)
        }
        Err(_) => {
            let _ = thread.join();
            Err("音声キャプチャスレッドの起動に失敗しました。".to_string())
        }
    }
}

/// ループバックキャプチャを停止する。
///
/// 実行中なら専用スレッドへ停止を通知し、join して Stream の drop 完了を待ってから
/// 状態を空へ戻す（この後すぐ再 Start 可能）。未開始の場合は panic せず `Ok(())` を返す。
#[tauri::command]
fn stop_audio_capture(state: tauri::State<CaptureState>) -> Result<(), String> {
    // join 中はロックを保持しないよう、ハンドルを取り出してからロックを解放する。
    let handle = {
        let mut handle_guard = state
            .handle
            .lock()
            .map_err(|_| "内部状態のロックに失敗しました。".to_string())?;
        handle_guard.take()
    };

    match handle {
        Some(handle) => {
            let _ = handle.stop_sender.send(());
            let _ = handle.thread.join();
            Ok(())
        }
        // 未開始でも安全に成功扱いにする（panic させない）。
        None => Ok(()),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(CaptureState::default())
        .invoke_handler(tauri::generate_handler![
            list_audio_devices,
            start_audio_capture,
            stop_audio_capture
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
