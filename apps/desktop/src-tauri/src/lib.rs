use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SizedSample, StreamConfig};
use std::sync::{mpsc, Mutex};
use std::thread;

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

/// Whisper 前処理で使う出力サンプルレート（16kHz モノラル）。
const TARGET_SAMPLE_RATE: u32 = 16_000;

/// モノラル音声を任意の入力レートから 16kHz へ変換する線形補間リサンプラー。
///
/// 方式B（位相アキュムレータ）: 入力を1サンプルずつ受け取り、常に「現在サンプルを 0、
/// 直前サンプルを -1」とする相対座標で次の出力位置 `next_out_offset` を保持する。
/// 新しい入力が来るたびに座標を1つずらす（-1.0）ことで、座標が無制限に増えず精度も安定する。
/// 状態はコールバック間で保持し、チャンク境界をまたいでも連続性が保たれる。
struct LinearResampler {
    /// 入力レート / 出力レート。1 出力あたりに進む入力サンプル数。
    step: f64,
    /// 次に生成する出力サンプルの位置（現在の入力サンプルを 0 とした相対座標）。
    next_out_offset: f64,
    /// 直前に受け取った入力サンプル（相対座標 -1 の値）。
    prev_sample: f64,
}

impl LinearResampler {
    /// 入力・出力レートからリサンプラーを作る。レートが 0 の場合は Err を返す。
    fn new(input_rate: u32, output_rate: u32) -> Result<Self, String> {
        if input_rate == 0 {
            return Err(
                "入力サンプルレートが 0 です。オーディオデバイスの設定を確認してください。"
                    .to_string(),
            );
        }
        if output_rate == 0 {
            return Err("出力サンプルレート(16000)が不正です。".to_string());
        }
        Ok(Self {
            step: input_rate as f64 / output_rate as f64,
            // 最初の入力サンプルで -1.0 されて 0 になり、出力位置 0（＝先頭サンプル）から生成する。
            next_out_offset: 1.0,
            prev_sample: 0.0,
        })
    }

    /// 入力モノラルサンプルを1つ処理し、生成された 16kHz サンプルを `on_output` へ渡す。
    ///
    /// 出力は f32（概ね -1.0..=1.0 に clamp 済み）。1サンプルにつき 0 個以上を生成する
    /// （ダウンサンプル時は数サンプルに1個、アップサンプル時は複数個）。
    fn process_sample<F: FnMut(f32)>(&mut self, sample: f64, mut on_output: F) {
        // 新しい入力サンプルが来たので、次出力位置を現在サンプル基準へ1つずらす。
        self.next_out_offset -= 1.0;
        // 直前サンプル(-1)と現在サンプル(0)の間に入る出力を線形補間で生成する。
        while self.next_out_offset <= 0.0 {
            let fraction = self.next_out_offset + 1.0; // 直前サンプルからの距離 [0.0, 1.0]
            let interpolated = self.prev_sample + (sample - self.prev_sample) * fraction;
            on_output((interpolated as f32).clamp(-1.0, 1.0));
            self.next_out_offset += self.step;
        }
        self.prev_sample = sample;
    }
}

/// ループバック用の入力ストリームを構築する（Whisper 前処理: モノラル化 + 16kHz リサンプリング検証）。
///
/// コールバックでは、インターリーブ入力をフレーム単位（1フレーム = 全チャンネル）でモノラル化し、
/// それを線形補間リサンプラーへ順番に渡して 16kHz へ変換、変換後サンプルの RMS を集計する。
/// 約1秒分（変換後 16000 サンプル）貯まるたびに設定と集計結果を表示し、集計値だけをリセットする
/// （リサンプラーの位相・前回サンプルなどの連続状態はリセットしない）。
///
/// サンプル形式(f32 / i16 / u16)ごとに同じ処理を使い回すためジェネリックにし、
/// 各形式を概ね -1.0〜1.0 に正規化する関数 `normalize` を引数で受け取る。
/// チャンネル数・サンプルレート・形式は固定せず、実際の StreamConfig から受け取る。
fn build_mono_monitoring_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    sample_format: SampleFormat,
    mut resampler: LinearResampler,
    output_rate: u32,
    normalize: fn(T) -> f64,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: SizedSample + 'static,
{
    // 実際のデバイス設定から取得する（48kHz/2ch を固定しない）。
    // チャンネル数が 0 でも chunks_exact(0) で panic しないよう最低 1 とする。
    let channels = config.channels.max(1) as usize;
    let input_rate = (config.sample_rate as usize).max(1);
    let format_label = format!("{sample_format:?}");
    // 表示判定は変換後サンプル数を基準にする。
    let report_threshold = (output_rate as usize).max(1);

    // ログ集計値。ストリーム生成のたびに新規初期化され、再 Start で持ち越さない。
    let mut input_sample_count: usize = 0;
    let mut mono_sample_count: usize = 0;
    let mut resampled_sample_count: usize = 0;
    let mut resampled_sum_of_squares: f64 = 0.0;

    device.build_input_stream::<T, _, _>(
        config,
        move |data: &[T], _info| {
            input_sample_count += data.len();

            // 1フレーム = channels 個。末尾の不完全フレームは chunks_exact が無視する。
            for frame in data.chunks_exact(channels) {
                let mut frame_sum = 0.0f64;
                for &sample in frame {
                    // 正規化後、安全のため -1.0..=1.0 に収める。
                    frame_sum += normalize(sample).clamp(-1.0, 1.0);
                }
                let mono = frame_sum / channels as f64;
                mono_sample_count += 1;

                // モノラルサンプルを 16kHz へ変換し、変換後サンプルだけを集計する。
                resampler.process_sample(mono, |resampled| {
                    let value = resampled as f64;
                    resampled_sum_of_squares += value * value;
                    resampled_sample_count += 1;
                });
            }

            // 変換後サンプル数（16kHz）を基準に約1秒ごとに表示する。
            if resampled_sample_count >= report_threshold {
                let resampled_rms = if resampled_sample_count > 0 {
                    (resampled_sum_of_squares / resampled_sample_count as f64).sqrt()
                } else {
                    0.0
                };
                println!("Input: {input_rate} Hz, {channels} ch, {format_label}");
                println!(
                    "Received input samples: {input_sample_count} | Mono samples: {mono_sample_count} | Resampled samples: {resampled_sample_count} | Resampled RMS: {resampled_rms:.6}"
                );

                input_sample_count = 0;
                mono_sample_count = 0;
                resampled_sample_count = 0;
                resampled_sum_of_squares = 0.0;
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

    // 開始時に実際に使用する設定を一度だけ表示する。
    println!(
        "Loopback config: {} Hz, {} channels, {:?}",
        config.sample_rate, config.channels, sample_format
    );

    // 実際の入力レートから 16kHz へのリサンプラーを作る（Start ごとに新規＝状態を持ち越さない）。
    // レートが不正な場合はここで分かりやすい Err を返す。
    let resampler = LinearResampler::new(config.sample_rate, TARGET_SAMPLE_RATE)?;

    let stream = match sample_format {
        // f32 はそのまま(概ね -1.0〜1.0)。
        SampleFormat::F32 => build_mono_monitoring_stream::<f32>(
            &device,
            &config,
            sample_format,
            resampler,
            TARGET_SAMPLE_RATE,
            |sample| sample as f64,
        ),
        // i16 は最大値で割って正規化する。
        SampleFormat::I16 => build_mono_monitoring_stream::<i16>(
            &device,
            &config,
            sample_format,
            resampler,
            TARGET_SAMPLE_RATE,
            |sample| sample as f64 / i16::MAX as f64,
        ),
        // u16 は中央値(32768)を 0 として正規化する。
        SampleFormat::U16 => build_mono_monitoring_stream::<u16>(
            &device,
            &config,
            sample_format,
            resampler,
            TARGET_SAMPLE_RATE,
            |sample| (sample as f64 - 32768.0) / 32768.0,
        ),
        other => {
            return Err(format!(
                "このデバイスのサンプル形式にはまだ対応していません: {other:?}"
            ))
        }
    }
    .map_err(|error| {
        format!("ループバック用の入力ストリームを作成できませんでした。詳細: {error}")
    })?;

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
