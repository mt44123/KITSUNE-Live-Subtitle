use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SizedSample, StreamConfig};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use whisper_rs::{
    install_logging_hooks, FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters,
    WhisperState,
};

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

/// オーディオコールバックからワーカーへ完成チャンクを渡す bounded channel の容量。
///
/// 有限容量にすることで、将来 Whisper 推論が入力より遅れてもキューが無制限に伸びず、
/// 保持する音声は「収集中の1チャンク + キュー内数チャンク + 処理中の1チャンク」に収まる。
const AUDIO_CHUNK_QUEUE_CAPACITY: usize = 2;

/// 1 チャンク（16kHz / mono / f32 / 16000 サンプル ≒ 1 秒）を Whisper で推論し、
/// 認識テキストをコンソールへ出す。
///
/// `state` はワーカーが所有し、チャンクごとに使い回す（毎チャンク作り直さない）。
/// whisper.cpp の `whisper_full_with_state` は呼び出しごとに内部の推論結果を上書き
/// するため、同じ `WhisperState` を連続して `full()` に渡して問題ない。各チャンクは
/// 独立した約1秒の音声なので、直前チャンクの結果をプロンプトに引きずらないよう
/// `set_no_context(true)` で毎回コンテキストをリセットする（rolling window ではない）。
///
/// `FullParams` は軽量なのでチャンクごとに新規生成する。入力 `samples` は借用のまま
/// 渡し、`Vec<f32>` の clone や再変換は行わない。
///
/// 失敗（推論失敗 / segment 取得失敗）は panic せず、呼び出し側が 1 チャンク分だけ
/// ログして次チャンクへ進めるよう、説明的な `Err(String)` を返す。
fn transcribe_chunk(state: &mut WhisperState, samples: &[f32]) -> Result<(), String> {
    // CPU 向け最小構成: greedy sampling（best_of = 1）。
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    // 認識のみ。英語への翻訳はしない。
    params.set_translate(false);
    // 言語は自動判定（日本語・英語のどちらも壊さない）。
    params.set_language(Some("auto"));
    // 各チャンクを独立扱いにし、直前チャンクのテキストをプロンプトに使わない。
    params.set_no_context(true);
    // タイムスタンプは不要。
    params.set_no_timestamps(true);
    // whisper.cpp 内部のデバッグ / 進捗 / リアルタイム出力を抑制する。
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_print_special(false);

    // 借用のまま渡す（clone しない）。full() 前の初期化は full() が内部で行う。
    state
        .full(params, samples)
        .map_err(|error| format!("Failed to transcribe audio chunk: {error}"))?;

    // 生成された全 segment のテキストを連結する。segment 数取得は c_int を直接返す
    // （Result ではない）ため、ここで失敗する経路はない。
    let segment_count = state.full_n_segments();
    let mut text = String::new();
    for index in 0..segment_count {
        let segment = state.get_segment(index).ok_or_else(|| {
            format!("Failed to read Whisper segment: index {index} out of bounds")
        })?;
        // 不正な UTF-8 は置換文字へ（日本語で途中バイトが来ても落とさない）。
        let segment_text = segment
            .to_str_lossy()
            .map_err(|error| format!("Failed to read Whisper segment: {error}"))?;
        text.push_str(&segment_text);
    }

    // 前後の空白を整理する。無音・空白のみのとき、および whisper.cpp が無音に対して
    // 返す特殊マーカー "[BLANK_AUDIO]" に完全一致するときは、不要ログを避けるため
    // 何も出さない。完全一致のみで判定し、部分一致で通常文章を消さない
    // （大文字小文字も区別する）。正常な英語・日本語の認識結果はそのまま残る。
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed == "[BLANK_AUDIO]" {
        return Ok(());
    }
    println!("Whisper transcription: {trimmed}");
    Ok(())
}

/// 受信した完成チャンクのサイズ・RMS をログし、Whisper モデルが利用可能なら推論して
/// 認識テキストを表示するオーディオワーカースレッドを起動する。
///
/// Whisper 推論は必ずこのワーカー側で実行し、リアルタイムな cpal コールバックでは
/// 行わない。`whisper_context` は Start 時に State から clone した `Arc` を受け取る
/// （`None` ならモデル未読み込み）。推論用 `WhisperState` はワーカー開始時に一度だけ
/// `create_state()` で作り、以後のチャンクで使い回す。
///
/// モデル未読み込み、または state 作成失敗のときは、開始時に 1 回だけ分かりやすい
/// ログを出して推論を無効化し、以降は従来どおりサイズ・RMS ログのみを続ける
/// （毎チャンク同じ警告は出さない）。1 チャンクの推論エラーではワーカー全体を落とさず、
/// そのチャンクだけログして次へ進む。
///
/// `receiver.iter()` は送信側（コールバック内の `SyncSender`）が drop されるまで
/// チャンクを1つずつ返し、drop（切断）後はキュー内の残りを受信し終えてから終了する
/// （推奨A: 停止時にキュー内の完成チャンクを取りこぼさない）。Stop 時に推論中でも、
/// 現在のチャンク推論が終わってから次の受信で切断を検知して抜けるため、join は
/// 有限時間で完了する。
fn spawn_audio_worker(
    chunk_receiver: mpsc::Receiver<Vec<f32>>,
    whisper_context: Option<Arc<WhisperContext>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        println!("Audio worker started");

        // 推論用 state はワーカー開始時に1回だけ作成し、以後再利用する。
        // context（Arc）はこのスレッドが所有し続けることで、state が参照する
        // 下層 whisper_context の生存を保証する。
        let mut whisper_state = match &whisper_context {
            Some(context) => match context.create_state() {
                Ok(state) => {
                    println!("Whisper transcription enabled");
                    Some(state)
                }
                // state 作成に失敗しても panic せず、推論だけ無効化して続行する。
                Err(error) => {
                    eprintln!("Failed to create Whisper state: {error}");
                    eprintln!("Whisper transcription disabled");
                    None
                }
            },
            None => {
                // モデル未読み込み。警告は開始時の1回だけ（毎チャンクは出さない）。
                println!("Whisper model is not loaded; transcription disabled");
                None
            }
        };

        // 送信側が生存する限りブロックして待ち、切断後は残りを受信して抜ける。
        for chunk in chunk_receiver.iter() {
            // RMS 計算はコールバック外（ワーカー側）で行い、リアルタイム性を確保する。
            let rms = chunk_rms(&chunk);
            println!("Worker received audio chunk: {} samples", chunk.len());
            println!("Worker chunk RMS: {rms:.6}");

            // モデルが利用可能なチャンクだけ推論する。エラーは1チャンク分ログして継続。
            if let Some(state) = whisper_state.as_mut() {
                if let Err(error) = transcribe_chunk(state, &chunk) {
                    eprintln!("{error}");
                }
            }
        }
        println!("Audio worker stopped");
    })
}

/// ループバック用の入力ストリームを構築する（Whisper 前処理: モノラル化 + 16kHz + 固定長チャンク化）。
///
/// コールバックでは、インターリーブ入力をフレーム単位（1フレーム = 全チャンネル）でモノラル化し、
/// 線形補間リサンプラーで 16kHz へ変換したサンプルを `AudioChunkBuffer` へ順番に push する。
/// CHUNK_SIZE(16000) 揃うたびに完成した `Vec<f32>` を `chunk_sender.try_send` でワーカーへ渡す。
///
/// リアルタイム性を守るため、コールバック内では RMS 計算やログ表示・重い処理・待機・
/// ブロッキング送信は行わない。送信はノンブロッキングな `try_send` のみで、
/// - 成功時: チャンクの所有権をワーカーへ移動する（clone しない）。
/// - `Full`: キューに空きがないので、そのチャンクをその場で破棄し1回だけログを出す。
/// - `Disconnected`: ワーカーが終了済み。破棄し、最初の1回だけログを出す（毎回は出さない）。
///
/// サンプル形式(f32 / i16 / u16)ごとに同じ処理を使い回すためジェネリックにし、
/// 各形式を概ね -1.0〜1.0 に正規化する関数 `normalize` を引数で受け取る。
/// チャンネル数・サンプルレートは固定せず、実際の StreamConfig から受け取る。
fn build_chunking_input_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    mut resampler: LinearResampler,
    chunk_sender: mpsc::SyncSender<Vec<f32>>,
    normalize: fn(T) -> f64,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: SizedSample + 'static,
{
    // チャンネル数が 0 でも chunks_exact(0) で panic しないよう最低 1 とする。
    let channels = config.channels.max(1) as usize;

    // 収集中のチャンクバッファ。ストリーム生成のたびに新規作成され、再 Start で持ち越さない。
    let mut chunk_buffer = AudioChunkBuffer::new();

    // Disconnected を毎チャンク記録しないための1回きりフラグ（move クロージャ内で保持）。
    let mut disconnect_logged = false;

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
                        // 完成チャンクはノンブロッキングでワーカーへ「移動」する。
                        // 成功時は clone せず所有権が移る。ここでは RMS もログも出さない。
                        match chunk_sender.try_send(chunk) {
                            Ok(()) => {}
                            // キュー満杯: 返ってきたチャンクをその場で破棄（drop）する。
                            Err(mpsc::TrySendError::Full(dropped)) => {
                                eprintln!(
                                    "Audio chunk queue full; dropping {}-sample chunk",
                                    dropped.len()
                                );
                            }
                            // ワーカー終了済み: 破棄し、最初の1回だけ通知する。
                            Err(mpsc::TrySendError::Disconnected(_)) => {
                                if !disconnect_logged {
                                    eprintln!(
                                        "Audio worker disconnected; dropping chunks until stop"
                                    );
                                    disconnect_logged = true;
                                }
                            }
                        }
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
///
/// `chunk_sender` は完成チャンクをワーカーへ渡すための bounded channel の送信側。
/// 成功時は Stream（内部のコールバック）が所有し、Stream が drop されると送信側も
/// drop されてワーカーが切断を検知する。失敗時はこの関数の終了時に drop される。
fn open_loopback_stream(chunk_sender: mpsc::SyncSender<Vec<f32>>) -> Result<cpal::Stream, String> {
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
        SampleFormat::F32 => build_chunking_input_stream::<f32>(
            &device,
            &config,
            resampler,
            chunk_sender,
            |sample| sample as f64,
        ),
        // i16 は最大値で割って正規化する。
        SampleFormat::I16 => build_chunking_input_stream::<i16>(
            &device,
            &config,
            resampler,
            chunk_sender,
            |sample| sample as f64 / i16::MAX as f64,
        ),
        // u16 は中央値(32768)を 0 として正規化する。
        SampleFormat::U16 => build_chunking_input_stream::<u16>(
            &device,
            &config,
            resampler,
            chunk_sender,
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
/// - `stop_sender`: キャプチャ用スレッドへ停止を通知するための送信側。
/// - `capture_thread`: Stream を所有するキャプチャ用スレッドの JoinHandle。
///   Stop 時に join して Stream（＝チャンク送信側）の drop 完了を待つ。
/// - `worker_thread`: チャンクを受信するワーカースレッドの JoinHandle。
///   キャプチャ側 drop 後に送信側が切断されるので、その後 join して終了を待つ。
struct CaptureHandle {
    stop_sender: mpsc::Sender<()>,
    capture_thread: thread::JoinHandle<()>,
    worker_thread: thread::JoinHandle<()>,
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
/// Start ごとに以下をすべて新規作成し、前回の状態を持ち越さない:
/// 新しい bounded channel、新しいワーカースレッド、新しい `AudioChunkBuffer`
/// （Stream 構築時）、新しい `LinearResampler`。
///
/// キャプチャ用スレッドを起動し、その中で Stream 生成と `play()` を行う。生成の成否は
/// setup チャンネルで受け取り、`play()` まで成功して初めて `Ok(())` を返す
/// （成功前に React 側を Capturing 状態にしないため）。
/// すでに実行中の場合は新しいスレッドを作らず、分かりやすいエラーを返す。
///
/// setup 失敗時は、キャプチャ用スレッド内で `chunk_sender` が drop されてワーカーが
/// 切断を検知するため、キャプチャ用スレッドとワーカースレッドの両方を join して
/// 中途半端なスレッドや State を残さない。
#[tauri::command]
fn start_audio_capture(
    state: tauri::State<CaptureState>,
    whisper_state: tauri::State<WhisperModelState>,
) -> Result<(), String> {
    let mut handle_guard = state
        .handle
        .lock()
        .map_err(|_| "内部状態のロックに失敗しました。".to_string())?;

    if handle_guard.is_some() {
        return Err("すでに音声キャプチャを実行中です。".to_string());
    }

    // Start 時点でモデルが読み込み済みなら、State から Arc<WhisperContext> だけを
    // clone する（モデル本体は複製せず参照カウントを +1 するだけ）。ここで Whisper
    // State の Mutex は即座に解放し、以降の推論中は保持しない。モデルのロードと
    // Start が競合しても、この短時間ロックは panic せず安全に処理される。
    // まだ読み込まれていなければ None を渡し、ワーカーは推論を無効化する。
    let whisper_context: Option<Arc<WhisperContext>> = {
        let guard = whisper_state
            .context
            .lock()
            .map_err(|_| "Whisper モデルの内部状態のロックに失敗しました。".to_string())?;
        guard.clone()
    };

    // 有限容量の bounded channel。容量を超える完成チャンクはコールバック側で破棄する。
    let (chunk_sender, chunk_receiver) = mpsc::sync_channel::<Vec<f32>>(AUDIO_CHUNK_QUEUE_CAPACITY);

    // 受信専用のワーカースレッド。送信側（Stream 内）が drop されるまで動き続ける。
    let worker_thread = spawn_audio_worker(chunk_receiver, whisper_context);

    let (stop_sender, stop_receiver) = mpsc::channel::<()>();
    let (setup_sender, setup_receiver) = mpsc::channel::<Result<(), String>>();

    let capture_thread = thread::spawn(move || match open_loopback_stream(chunk_sender) {
        Ok(stream) => {
            // 生成・再生成功を通知したうえで、停止通知が来るまで待機する。
            let _ = setup_sender.send(Ok(()));
            // recv() は () を受信するか、送信側が drop されると返る。どちらでも停止する。
            let _ = stop_receiver.recv();
            // ここで stream が drop され、キャプチャが停止する。
            // 同時にコールバック内の chunk_sender も drop され、ワーカーが切断を検知する。
            drop(stream);
        }
        Err(message) => {
            // 失敗時は chunk_sender がここで drop され、ワーカーが終了できる。
            let _ = setup_sender.send(Err(message));
        }
    });

    // Stream の生成と play() の成功を確認してから成功扱いにする。
    match setup_receiver.recv() {
        Ok(Ok(())) => {
            *handle_guard = Some(CaptureHandle {
                stop_sender,
                capture_thread,
                worker_thread,
            });
            Ok(())
        }
        Ok(Err(message)) => {
            // 失敗時、キャプチャ用スレッドは終了済み。送信側 drop によりワーカーも終了する。
            let _ = capture_thread.join();
            let _ = worker_thread.join();
            Err(message)
        }
        Err(_) => {
            let _ = capture_thread.join();
            let _ = worker_thread.join();
            Err("音声キャプチャスレッドの起動に失敗しました。".to_string())
        }
    }
}

/// ループバックキャプチャを停止する。
///
/// 実行中なら以下の順で安全に終了させ、状態を空へ戻す（この後すぐ再 Start 可能）:
/// 1. キャプチャ用スレッドへ停止通知を送る。
/// 2. キャプチャ用スレッドを join する（内部で Stream を drop → コールボック内の
///    chunk_sender も drop され、ワーカーの受信側が切断される）。
/// 3. ワーカースレッドを join する（切断検知後、キュー内の残りチャンクを受信し終えて終了）。
///
/// この順序により「送信側が生きたままワーカーを先に join して永久待機」する
/// デッドロックを避ける。未開始の場合は panic せず `Ok(())` を返す。
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
            // 先にキャプチャ用スレッドを join して送信側を確実に drop させる。
            let _ = handle.capture_thread.join();
            // その後ワーカーを join する。切断済みなので残りを処理して終了する。
            let _ = handle.worker_thread.join();
            Ok(())
        }
        // 未開始でも安全に成功扱いにする（panic させない）。
        None => Ok(()),
    }
}

// ===========================================================================
// Whisper モデル読み込み（Phase 3 の最初のステップ）
//
// この節は「手動配置された Whisper モデルを一度だけ安全に読み込み、結果を UI へ
// 返す」ことだけを担当する。音声チャンクの推論・文字起こしはまだ行わず、既存の
// 音声キャプチャ処理（WASAPI ループバック / モノラル化 / リサンプル / チャンク化 /
// bounded channel / ワーカー）とは完全に独立している。audio worker は Whisper
// context へアクセスしない。
// ===========================================================================

/// 読み込む Whisper モデルのファイル名。今回は base モデルを想定する。
const WHISPER_MODEL_FILE_NAME: &str = "ggml-base.bin";

/// リポジトリ内の Whisper モデルファイルの絶対パスを解決する。
///
/// PC 固有の絶対パス（例: `C:\Users\...`）をソースへ書かないため、コンパイル時に
/// 確定する `CARGO_MANIFEST_DIR`（= この crate の `src-tauri` ディレクトリ）を
/// 基準に `models/ggml-base.bin` を指す。開発環境（`tauri dev`）ではこの相対配置
/// で確実に読み込める。
///
/// 将来の配布時は Tauri の resource ディレクトリ等へ切り替える余地があるが、今回は
/// 開発環境で確実に読めることを優先し、リポジトリ内相対パスのみを解決する。
fn resolve_whisper_model_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("models")
        .join(WHISPER_MODEL_FILE_NAME)
}

/// Tauri State として保持する Whisper モデルの状態。
///
/// 読み込み済みの `WhisperContext` を `Arc` で包み、`Mutex<Option<...>>` で保持する
/// （`None` なら未読み込み、`Some` なら読み込み済み）。`Arc` にすることで、Start 時に
/// State のロックを短時間だけ取って `Arc` の複製（＝参照カウント +1、モデル本体は複製
/// しない）を取り出し、以降はロックを解放したままワーカースレッドが Whisper context を
/// 利用できる。推論中に State の Mutex を保持し続けない設計にするための包み方。
///
/// `WhisperContext` は whisper-rs 側でスレッド安全に実装されている（`Send` + `Sync`）
/// ため、この crate 側で `unsafe impl Send/Sync` や `static mut`、生ポインタ共有を
/// 使わずに、そのまま Tauri State（内部で `Send + Sync` を要求）へ安全に保持できる。
#[derive(Default)]
struct WhisperModelState {
    context: Mutex<Option<Arc<WhisperContext>>>,
}

/// 手動配置された Whisper モデルを読み込み、Tauri State へ保存する。
///
/// 処理の流れ:
/// 1. すでに読み込み済みなら再ロードせず「既に読み込み済み」を成功として返す。
/// 2. モデルパスを解決し、ファイルの存在を確認する（無ければ分かりやすい Err）。
/// 3. `spawn_blocking` 上で `WhisperContext` を生成する（重い処理で UI/メイン
///    スレッドをブロックしないため）。
/// 4. 生成した context を State へ保存する。
///
/// `unwrap()` や `panic!` でアプリを終了させず、失敗はすべて文字列 Err で返す。
/// 非同期コマンドにすることで、読み込み中も Tauri のメインスレッドを塞がない。
#[tauri::command]
async fn load_whisper_model(state: tauri::State<'_, WhisperModelState>) -> Result<String, String> {
    // すでに読み込み済みなら再ロードしない（メモリの無駄遣いと二重ロードを防ぐ）。
    // ロックは await をまたいで保持しないよう、ここで一旦解放する。
    {
        let guard = state
            .context
            .lock()
            .map_err(|_| "Whisper モデルの内部状態のロックに失敗しました。".to_string())?;
        if guard.is_some() {
            return Ok("Whisper model is already loaded".to_string());
        }
    }

    let model_path = resolve_whisper_model_path();
    if !model_path.exists() {
        return Err(format!(
            "Whisper model file not found: {}",
            model_path.display()
        ));
    }

    // モデル読み込みは重い可能性があるため、ブロッキング専用スレッドで生成する。
    // 生成した WhisperContext は Send なのでスレッドをまたいで受け取れる。
    let context = tauri::async_runtime::spawn_blocking(move || {
        // 今回は CPU 実行のみ。GPU は明示的に無効化する。
        let parameters = WhisperContextParameters {
            use_gpu: false,
            ..Default::default()
        };
        WhisperContext::new_with_params(&model_path, parameters)
    })
    .await
    .map_err(|error| format!("Failed to load Whisper model: {error}"))?
    .map_err(|error| format!("Failed to load Whisper model: {error}"))?;

    // 読み込み中に別の呼び出しが先に完了していた場合は、そちらを尊重して破棄する。
    let mut guard = state
        .context
        .lock()
        .map_err(|_| "Whisper モデルの内部状態のロックに失敗しました。".to_string())?;
    if guard.is_some() {
        return Ok("Whisper model is already loaded".to_string());
    }
    // Arc で包んで保持する。以後 Start 時はこの Arc を clone して共有する。
    *guard = Some(Arc::new(context));
    Ok("Whisper model loaded successfully".to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // whisper.cpp / GGML の内部ログ（whisper_full_with_state: ... や seek = ... など）を
    // whisper-rs の安全な公開 API で抑制する。log_backend / tracing_backend feature を
    // 有効化していないため、これらのログはどこにも出力されなくなる（実質的に無効化）。
    // アプリ側で明示的に出す println!/eprintln!（RMS・認識結果・エラー等）は影響を受けない。
    // この関数は「複数回呼んでも安全・効果は初回のみ」だが、意図を明確にするため
    // アプリ起動時にここで一度だけ呼ぶ。unsafe な set_log_callback や独自 C callback は使わない。
    install_logging_hooks();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(CaptureState::default())
        .manage(WhisperModelState::default())
        .invoke_handler(tauri::generate_handler![
            list_audio_devices,
            start_audio_capture,
            stop_audio_capture,
            load_whisper_model
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
