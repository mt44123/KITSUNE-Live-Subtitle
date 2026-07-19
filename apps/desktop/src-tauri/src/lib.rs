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

/// Whisper へ渡す 1 チャンクのサンプル数（16kHz モノラルで約1秒分）。
const CHUNK_SIZE: usize = 16_000;

/// 16kHz モノラルサンプルを固定長（CHUNK_SIZE）まで貯めるバッファ。
///
/// `push` で 1 サンプルずつ追加し、CHUNK_SIZE 揃ったら `Vec<f32>` を返す。
/// 収集中は最大でも CHUNK_SIZE 個しか保持しないため、メモリは増え続けない。
struct AudioChunkBuffer {
    samples: Vec<f32>,
}

impl AudioChunkBuffer {
    fn new() -> Self {
        Self {
            samples: Vec::with_capacity(CHUNK_SIZE),
        }
    }

    /// サンプルを 1 つ追加する。CHUNK_SIZE 揃ったら完成チャンクを返し、収集を新しく始める。
    ///
    /// 完成時は `std::mem::take` で内部 Vec の所有権を呼び出し元へ移す（clone しない）。
    /// take 後は空の Vec になるため、次チャンク用に容量を確保し直す。
    fn push(&mut self, sample: f32) -> Option<Vec<f32>> {
        self.samples.push(sample);
        if self.samples.len() >= CHUNK_SIZE {
            let chunk = std::mem::take(&mut self.samples);
            self.samples = Vec::with_capacity(CHUNK_SIZE);
            Some(chunk)
        } else {
            None
        }
    }
}

/// 完成した 16kHz チャンクの RMS を計算する。チャンク完成時だけ呼ぶ。
fn chunk_rms(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_of_squares: f64 = samples
        .iter()
        .map(|&sample| {
            let value = sample as f64;
            value * value
        })
        .sum();
    (sum_of_squares / samples.len() as f64).sqrt()
}

/// ループバック用の入力ストリームを構築する（Whisper 前処理: モノラル化 + 16kHz + 固定長チャンク化）。
///
/// コールバックでは、インターリーブ入力をフレーム単位（1フレーム = 全チャンネル）でモノラル化し、
/// 線形補間リサンプラーで 16kHz へ変換したサンプルを `AudioChunkBuffer` へ順番に push する。
/// CHUNK_SIZE(16000) 揃うたびに `Vec<f32>` を取り出して RMS を表示する（チャンク自体は保持しない）。
///
/// サンプル形式(f32 / i16 / u16)ごとに同じ処理を使い回すためジェネリックにし、
/// 各形式を概ね -1.0〜1.0 に正規化する関数 `normalize` を引数で受け取る。
/// チャンネル数・サンプルレートは固定せず、実際の StreamConfig から受け取る。
fn build_chunking_input_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    mut resampler: LinearResampler,
    normalize: fn(T) -> f64,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: SizedSample + 'static,
{
    // チャンネル数が 0 でも chunks_exact(0) で panic しないよう最低 1 とする。
    let channels = config.channels.max(1) as usize;

    // 収集中のチャンクバッファ。ストリーム生成のたびに新規作成され、再 Start で持ち越さない。
    let mut chunk_buffer = AudioChunkBuffer::new();

    device.build_input_stream::<T, _, _>(
        config,
        move |data: &[T], _info| {
            // 1フレーム = channels 個。末尾の不完全フレームは chunks_exact が無視する。
            for frame in data.chunks_exact(channels) {
                let mut frame_sum = 0.0f64;
                for &sample in frame {
                    // 正規化後、安全のため -1.0..=1.0 に収める。
                    frame_sum += normalize(sample).clamp(-1.0, 1.0);
                }
                let mono = frame_sum / channels as f64;

                // モノラルサンプルを 16kHz へ変換し、チャンクへ push する。
                resampler.process_sample(mono, |resampled| {
                    if let Some(chunk) = chunk_buffer.push(resampled) {
                        // チャンク完成時だけログを出す（毎サンプルの println はしない）。
                        // 将来はここで chunk を Whisper へ渡す。今は表示のみで破棄する。
                        let rms = chunk_rms(&chunk);
                        println!("Generated audio chunk: {} samples", chunk.len());
                        println!("Chunk RMS: {rms:.6}");
                    }
                });
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
        SampleFormat::F32 => {
            build_chunking_input_stream::<f32>(&device, &config, resampler, |sample| sample as f64)
        }
        // i16 は最大値で割って正規化する。
        SampleFormat::I16 => {
            build_chunking_input_stream::<i16>(&device, &config, resampler, |sample| {
                sample as f64 / i16::MAX as f64
            })
        }
        // u16 は中央値(32768)を 0 として正規化する。
        SampleFormat::U16 => {
            build_chunking_input_stream::<u16>(&device, &config, resampler, |sample| {
                (sample as f64 - 32768.0) / 32768.0
            })
        }
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
