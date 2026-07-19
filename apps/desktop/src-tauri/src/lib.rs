use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SizedSample, StreamConfig};
use std::sync::mpsc;
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
/// コールバックでは受け取ったサンプル数を数え、約1秒ごとに合計を表示する。
/// （毎コールバックで表示すると大量に出力されるため、1秒間隔にまとめている。）
/// サンプル形式（f32 / i16 / u16）ごとに同じ処理を使い回すためジェネリックにしている。
fn build_counting_input_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: SizedSample,
{
    let mut sample_count: usize = 0;
    let mut last_report = Instant::now();

    device.build_input_stream::<T, _, _>(
        config,
        move |data: &[T], _info| {
            sample_count += data.len();
            if last_report.elapsed() >= Duration::from_secs(1) {
                println!("Received samples: {sample_count}");
                sample_count = 0;
                last_report = Instant::now();
            }
        },
        move |error| eprintln!("音声ストリームでエラーが発生しました: {error}"),
        None,
    )
}

/// 既定の出力デバイスを WASAPI ループバックとして取得し、サンプル受信を開始する。
///
/// cpal では「出力デバイスに対して build_input_stream を呼ぶ」とループバックになる。
/// 今回は Stop を実装しないため、ストリームは専用スレッドで保持し続ける
/// （スレッドが生きている間はキャプチャが続く）。cpal の Stream はスレッドを
/// またいで送れないため、デバイス取得からストリーム生成まで同じスレッドで行う。
#[tauri::command]
fn start_audio_capture() -> Result<(), String> {
    let (setup_result_sender, setup_result_receiver) = mpsc::channel::<Result<(), String>>();

    thread::spawn(move || {
        let stream_setup = (|| -> Result<cpal::Stream, String> {
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
                SampleFormat::F32 => build_counting_input_stream::<f32>(&device, &config),
                SampleFormat::I16 => build_counting_input_stream::<i16>(&device, &config),
                SampleFormat::U16 => build_counting_input_stream::<u16>(&device, &config),
                other => {
                    return Err(format!(
                        "このデバイスのサンプル形式にはまだ対応していません: {other:?}"
                    ))
                }
            }
            .map_err(|error| {
                format!("ループバック用の入力ストリームを作成できませんでした。詳細: {error}")
            })?;

            stream.play().map_err(|error| {
                format!("音声ストリームを開始できませんでした。詳細: {error}")
            })?;

            Ok(stream)
        })();

        match stream_setup {
            Ok(stream) => {
                // セットアップ成功を呼び出し元へ通知する。
                let _ = setup_result_sender.send(Ok(()));
                // Stop 機能が無い今回は、ストリームを保持したまま待機し続ける。
                loop {
                    thread::sleep(Duration::from_secs(60));
                    // `stream` を drop させないためにここで所有し続ける。
                    let _keep_alive = &stream;
                }
            }
            Err(message) => {
                let _ = setup_result_sender.send(Err(message));
            }
        }
    });

    match setup_result_receiver.recv() {
        Ok(result) => result,
        Err(_) => Err("音声キャプチャスレッドの起動に失敗しました。".to_string()),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            list_audio_devices,
            start_audio_capture
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
