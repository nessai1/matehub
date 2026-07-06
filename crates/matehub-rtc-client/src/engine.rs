//! Поточный раннер sans-IO ядра.
//!
//! Два выделенных std-потока:
//! - `udp-rx` — блокирующий `recv_from`, пересылает датаграммы в общий
//!   data-канал (bounded: буфер и так дублирует kernel receive buffer);
//! - `rtc-engine` — цикл str0m: drain `poll_output` до `Timeout`, затем
//!   `crossbeam::select!` по control/data каналам с дедлайном.
//!
//! Контракт str0m «после каждой мутации — drain до Timeout» соблюдается
//! конструкцией цикла: каждая ветка select делает ровно одну мутацию и
//! возвращается к drain.

use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use crossbeam::channel::{Receiver, Sender, bounded, unbounded};
use tokio::sync::mpsc::UnboundedSender;

use crate::core::{CoreOut, CorePoll, EngineCore};
use crate::signaling::{ClientMessage, ServerMessage};
use crate::types::{EncodedAudioFrame, EncodedVideoFrame, PublisherEvent};

/// Управляющие команды — терять нельзя. Канал unbounded: сигналинг штучный,
/// а blocking send из tokio-таски (WS reader) недопустим.
pub(crate) enum CtrlCmd {
    Signal(ServerMessage),
    PublishScreen { audio: bool },
    PublishMicrophone,
    Stop,
}

/// Данные — допускают дроп под давлением.
pub(crate) enum DataCmd {
    Udp { source: SocketAddr, buf: Vec<u8> },
    Frame(EncodedVideoFrame),
    Audio(EncodedAudioFrame),
}

pub(crate) struct EngineChannels {
    pub ctrl_tx: Sender<CtrlCmd>,
    pub data_tx: Sender<DataCmd>,
}

/// Ёмкость data-канала: при 60 fps + RTCP-трафике очередь в тысячи слотов
/// означает уже сломанный пайплайн — лучше дропнуть и увидеть в логах.
const DATA_CHANNEL_CAP: usize = 2048;

/// Максимальный сон между тиками: str0m может отдать дедлайн далеко в
/// будущем, но нам нужно регулярно проверять is_alive и каналы.
const MAX_SLEEP: Duration = Duration::from_millis(500);

pub(crate) fn spawn(
    core: EngineCore,
    socket: UdpSocket,
    local_addr: SocketAddr,
    ws_tx: UnboundedSender<ClientMessage>,
    event_tx: UnboundedSender<PublisherEvent>,
) -> std::io::Result<EngineChannels> {
    let (ctrl_tx, ctrl_rx) = unbounded::<CtrlCmd>();
    let (data_tx, data_rx) = bounded::<DataCmd>(DATA_CHANNEL_CAP);

    // udp-rx: сокет клонируется, engine оставляет себе send-половину.
    let recv_socket = socket.try_clone()?;
    let udp_data_tx = data_tx.clone();
    std::thread::Builder::new()
        .name("matehub-rtc-udp-rx".into())
        .spawn(move || {
            let mut buf = vec![0u8; 2048];
            loop {
                match recv_socket.recv_from(&mut buf) {
                    Ok((n, source)) => {
                        let cmd = DataCmd::Udp {
                            source,
                            buf: buf[..n].to_vec(),
                        };
                        // Engine умер → выходим вместе с ним.
                        if udp_data_tx.send(cmd).is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        tracing::warn!("udp recv error: {e}");
                        return;
                    }
                }
            }
        })?;

    std::thread::Builder::new()
        .name("matehub-rtc-engine".into())
        .spawn(move || {
            run_engine(core, socket, local_addr, ctrl_rx, data_rx, ws_tx, event_tx);
        })?;

    Ok(EngineChannels { ctrl_tx, data_tx })
}

fn run_engine(
    mut core: EngineCore,
    socket: UdpSocket,
    local_addr: SocketAddr,
    ctrl_rx: Receiver<CtrlCmd>,
    data_rx: Receiver<DataCmd>,
    ws_tx: UnboundedSender<ClientMessage>,
    event_tx: UnboundedSender<PublisherEvent>,
) {
    let dispatch = |outs: Vec<CoreOut>| -> bool {
        for out in outs {
            let alive = match out {
                CoreOut::SendSignal(msg) => ws_tx.send(msg).is_ok(),
                CoreOut::Emit(event) => event_tx.send(event).is_ok(),
            };
            if !alive {
                // Publisher дропнут — работать больше не для кого.
                return false;
            }
        }
        true
    };

    let mut dropped_frames: u64 = 0;

    loop {
        if !core.is_alive() {
            let _ = event_tx.send(PublisherEvent::Disconnected {
                reason: "rtc closed".into(),
            });
            return;
        }

        // Drain до Timeout — единственная точка, где str0m отдаёт дедлайн.
        let deadline = loop {
            match core.poll() {
                CorePoll::Transmit {
                    destination,
                    contents,
                } => {
                    if let Err(e) = socket.send_to(&contents, destination) {
                        // Транзиентные ошибки (ENETUNREACH при смене сети)
                        // переживаем: ICE сам решит, что кандидат мёртв.
                        tracing::debug!("udp send to {destination}: {e}");
                    }
                }
                CorePoll::Outs(outs) => {
                    if !dispatch(outs) {
                        return;
                    }
                }
                CorePoll::Timeout(deadline) => break deadline,
                CorePoll::Fatal(reason) => {
                    let _ = event_tx.send(PublisherEvent::Disconnected { reason });
                    return;
                }
            }
        };

        let now = Instant::now();
        let timeout = deadline.saturating_duration_since(now).min(MAX_SLEEP);

        crossbeam::channel::select! {
            recv(ctrl_rx) -> cmd => match cmd {
                Ok(CtrlCmd::Signal(msg)) => {
                    if !dispatch(core.handle_signal(msg)) {
                        return;
                    }
                }
                Ok(CtrlCmd::PublishScreen { audio }) => {
                    if !dispatch(core.publish_screen(audio)) {
                        return;
                    }
                }
                Ok(CtrlCmd::PublishMicrophone) => {
                    if !dispatch(core.publish_microphone()) {
                        return;
                    }
                }
                Ok(CtrlCmd::Stop) | Err(_) => {
                    let _ = ws_tx.send(ClientMessage::Leave);
                    return;
                }
            },
            recv(data_rx) -> cmd => match cmd {
                Ok(DataCmd::Udp { source, buf }) => {
                    core.handle_udp(Instant::now(), source, local_addr, &buf);
                }
                Ok(DataCmd::Frame(frame)) => {
                    if !core.write_frame(&frame) {
                        dropped_frames += 1;
                        if dropped_frames.is_power_of_two() {
                            tracing::debug!(dropped_frames, "frame dropped (negotiation in flight?)");
                        }
                    }
                }
                Ok(DataCmd::Audio(frame)) => {
                    // Аудио дропаем тихо: 20мс пакеты, потеря одного не рвёт
                    // поток так, как видео (там кадр = целый AU).
                    let _ = core.write_audio(&frame);
                }
                Err(_) => return,
            },
            default(timeout) => {
                core.handle_timeout(Instant::now());
            }
        }
    }
}
