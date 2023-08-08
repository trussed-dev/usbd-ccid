use core::convert::TryFrom;
use heapless::Vec;

use crate::{
    constants::*,
    types::packet::{
        Chain, ChainedPacket as _, Command as PacketCommand, DataBlock, Error as PacketError,
        ExtPacket, PacketWithData as _, RawPacket, RawPacketExt as _, XfrBlock,
    },
};

use usb_device::class_prelude::*;

#[allow(clippy::assertions_on_constants)]
const _: () = assert!(MAX_MSG_LENGTH >= PACKET_SIZE);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum State {
    Idle,
    Receiving,
    Processing,
    ReadyToSend,
    Sending,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[allow(dead_code, clippy::enum_variant_names)]
enum Error {
    CmdAborted = 0xff,
    IccMute = 0xfe,
    XfrParityError = 0xfd,
    //..
    CmdSlotBusy = 0xE0,
    CommandNotSupported = 0x00,
}

pub(crate) type Requester<'pipe, const N: usize> =
    interchange::Requester<'pipe, iso7816::Data<N>, iso7816::Data<N>>;

pub struct Pipe<'bus, 'pipe, Bus, const N: usize>
where
    Bus: 'static + UsbBus,
{
    pub(crate) write: EndpointIn<'bus, Bus>,
    // pub(crate) rpc: TransportEndpoint<'rpc>,
    seq: u8,
    state: State,
    interchange: Requester<'pipe, N>,
    sent: usize,
    outbox: Option<RawPacket>,

    ext_packet: ExtPacket,
    #[allow(dead_code)]
    packet_len: usize,
    receiving_long: bool,
    long_packet_missing: usize,
    in_chain: usize,
    pub(crate) started_processing: bool,
    atr: Vec<u8, 32>,
    // The sequence number of the last bulk command if it was an abort command.
    bulk_abort: Option<u8>,
    // The sequence number of the last abort command received over the control pipe, if any.
    control_abort: Option<u8>,
}

impl<'bus, 'pipe, Bus, const N: usize> Pipe<'bus, 'pipe, Bus, N>
where
    Bus: 'static + UsbBus,
{
    pub(crate) fn new(
        write: EndpointIn<'bus, Bus>,
        request_pipe: Requester<'pipe, N>,
        card_issuers_data: Option<&[u8]>,
    ) -> Self {
        Self {
            write,
            seq: 0,
            state: State::Idle,
            sent: 0,
            outbox: None,
            interchange: request_pipe,

            ext_packet: Default::default(),
            packet_len: 0,
            receiving_long: false,
            long_packet_missing: 0,
            in_chain: 0,
            started_processing: false,
            // later on, we only signal T=1 support
            // if for some reason not signaling T=0 support leads to issues,
            // we can enable it here.
            atr: Self::construct_atr(card_issuers_data, false),
            bulk_abort: None,
            control_abort: None,
        }
    }

    /// Reset the state of the CCID driver
    ///
    /// This is done on unexpected input instead of panicking
    pub fn reset_state(&mut self) {
        self.seq = 0;
        self.state = State::Idle;
        self.sent = 0;
        self.outbox = None;
        self.packet_len = 0;
        self.receiving_long = false;
        self.long_packet_missing = 0;
        self.in_chain = 0;
        self.started_processing = false;
        self.bulk_abort = None;
        self.control_abort = None;
        self.reset_interchange();
    }

    fn construct_atr(card_issuers_data: Option<&[u8]>, signal_t_equals_0: bool) -> Vec<u8, 32> {
        assert!(card_issuers_data.map_or(true, |data| data.len() <= 13));
        let k = card_issuers_data.map_or(0u8, |data| 2 + data.len() as u8);
        let mut atr = Vec::new();
        // TS: direct convention
        atr.push(0x3B).ok();
        // T0: encode length of historical bytes
        atr.push(0x80 | k).ok();
        if signal_t_equals_0 {
            // T=0, more to follow
            atr.push(0x80).ok();
        }
        // T=1
        atr.push(0x01).ok();

        if let Some(data) = card_issuers_data {
            // no status indicator
            atr.push(0x80).ok();
            // tag 5: card issuer's data
            atr.push(0x50 | data.len() as u8).ok();
            atr.extend_from_slice(data).ok();
        }
        // xor of all bytes except TS
        let mut checksum = 0;
        for byte in atr.iter().skip(1) {
            checksum ^= *byte;
        }
        atr.push(checksum).ok();

        atr
    }

    pub fn handle_packet(&mut self, packet: RawPacket) {
        use crate::types::packet::RawPacketExt;

        // SHOULD CLEAN THIS UP!
        // The situation is as follows: full 64B USB packet received.
        // CCID packet signals no command chaining, but data length > 64 - 10.
        // Then we can expect to receive more USB packets containing only data.
        // The concatenation of all these is then a valid Command APDU.
        // (which itself may have command chaining on a higher level, e.g.
        // when certificates are transmitted, because PIV somehow uses short APDUs
        // only (can we fix this), so 255B is the maximum)
        if !self.receiving_long {
            if packet.len() < CCID_HEADER_LEN {
                error!("unexpected short packet");
                self.reset_state();
                return;
            }
            self.ext_packet.clear();
            // TODO check
            self.ext_packet
                .extend_from_slice(&packet)
                .expect("Raw packets are not larger than ext packets");

            let pl = packet.data_len();
            if pl > PACKET_SIZE - CCID_HEADER_LEN {
                self.receiving_long = true;
                self.in_chain = 1;
                self.long_packet_missing = pl - (PACKET_SIZE - CCID_HEADER_LEN);
                self.packet_len = pl;
                return;
            }
        } else {
            // TODO check
            if self.ext_packet.extend_from_slice(&packet).is_err() {
                error!(
                    "Extended packet got larger than maximum size ({}), wants {}",
                    self.ext_packet.capacity(),
                    self.ext_packet.len() + packet.len(),
                );
                self.reset_state();
                return;
            }
            self.in_chain += 1;
            if packet.len() > self.long_packet_missing {
                error!("Got larger packet than expected");
                self.long_packet_missing = 0;
            } else {
                self.long_packet_missing -= packet.len();
            }
            if self.long_packet_missing != 0 {
                return;
            }

            // info!("pl {}, p {}, missing {}, in_chain {}", self.packet_len, packet.len(), self.long_packet_missing, self.in_chain).ok();
            // info!("packet: {:X?}", &self.ext_packet).ok();
            self.receiving_long = false;
        }

        // info!("{:X?}", &packet).ok();
        // let p = packet.clone();
        // match PacketCommand::try_from(packet) {
        match PacketCommand::try_from(self.ext_packet.clone()) {
            Ok(command) => {
                self.seq = command.seq();

                // If we receive an ABORT on the control pipe, we reject all further commands until
                // we receive a matching ABORT on the bulk endpoint too.
                if let Some(control_abort) = self.control_abort {
                    if matches!(command, PacketCommand::Abort(_)) && control_abort == self.seq {
                        self.abort();
                    } else {
                        self.send_slot_status_error(Error::CmdAborted);
                    }
                    return;
                }
                self.bulk_abort = None;

                // happy path
                match command {
                    PacketCommand::PowerOn(_command) => self.send_atr(),

                    PacketCommand::PowerOff(_command) => self.send_slot_status_ok(),

                    PacketCommand::GetSlotStatus(_command) => self.send_slot_status_ok(),

                    PacketCommand::XfrBlock(command) => self.handle_transfer(command),

                    PacketCommand::Abort(_command) => self.bulk_abort = Some(self.seq),

                    PacketCommand::GetParameters(_command) => self.send_parameters(),
                }
            }

            Err(PacketError::ShortPacket) => {
                error!("Unexpectedly short packet");
                self.reset_state();
            }

            Err(PacketError::UnknownCommand(_p)) => {
                info!("unknown command {:X?}", &_p);
                self.seq = self.ext_packet[6];
                self.send_slot_status_error(Error::CommandNotSupported);
            }
        }
    }

    #[inline(never)]
    fn reset_interchange(&mut self) {
        let message = Vec::new();
        // this may no longer be needed
        // before the interchange change (adding the request_mut method),
        // one necessary side-effect of this was to set the interchange's
        // enum variant to Request.
        self.interchange.request(message).ok();
        self.interchange.cancel().ok();

        self.interchange.take_response();
    }

    fn handle_transfer(&mut self, command: XfrBlock) {
        // state: Idle, Receiving, Processing, Sending,
        //
        // conts: BeginsAndEnds, Begins, Ends, Continues, ExpectDataBlock,

        // info!("handle xfrblock").ok();
        // info!("{:X?}", &command);
        match self.state {
            State::Idle => {
                // invariant: BUFFER_SIZE >= PACKET_SIZE
                match command.chain() {
                    Ok(Chain::BeginsAndEnds) => {
                        info!("begins and ends");
                        self.reset_interchange();
                        let Ok(message) = self.interchange.request_mut() else {
                            error!("Interchange is busy");
                            self.reset_state();
                            return;
                        };
                        message.clear();
                        if message.extend_from_slice(command.data()).is_err() {
                            error!("Interchange is full");
                            self.reset_state();
                            return;
                        };
                        self.call_app();
                        self.state = State::Processing;
                        // self.send_empty_datablock();
                    }
                    Ok(Chain::Begins) => {
                        info!("begins");
                        self.reset_interchange();
                        let Ok(message) = self.interchange.request_mut() else {
                            error!("Interchange is busy");
                            self.reset_state();
                            return;
                        };
                        message.clear();
                        if message.extend_from_slice(command.data()).is_err() {
                            error!("Interchange is full");
                            self.reset_state();
                            return;
                        };
                        self.state = State::Receiving;
                        self.send_empty_datablock(Chain::ExpectingMore);
                    }
                    Err(_) => {
                        error!("Unknown chain");
                        self.reset_state();
                    }
                    _ => {
                        error!("unexpectedly in idle state");
                        self.reset_state();
                    }
                }
            }

            State::Receiving => match command.chain() {
                Ok(Chain::Continues) => {
                    info!("continues");
                    let Ok(message) = self.interchange.request_mut() else {
                        error!("Interchange is busy");
                        self.reset_state();
                        return;
                    };
                    if message.extend_from_slice(command.data()).is_err() {
                        error!("Receiving unexpectedly large data");
                        self.reset_state();
                        return;
                    }
                    self.send_empty_datablock(Chain::ExpectingMore);
                }
                Ok(Chain::Ends) => {
                    info!("ends");
                    let Ok(message) = self.interchange.request_mut() else {
                        error!("Interchange is busy");
                        self.reset_state();
                        return;
                    };
                    if message.extend_from_slice(command.data()).is_err() {
                        error!("Receiving unexpectedly large data");
                        self.reset_state();
                        return;
                    }
                    self.call_app();
                    self.state = State::Processing;
                }
                Err(_) => {
                    error!("Unknown chain");
                    self.reset_state();
                }
                _ => {
                    error!("unexpectedly in receiving state");
                    self.reset_state();
                }
            },

            State::Processing | State::ReadyToSend => {
                error!(
                    "ccid pipe unexpectedly received command {:?} while in state: {:?}",
                    &command, self.state,
                );
                self.reset_state();
            }

            State::Sending => match command.chain() {
                Ok(Chain::ExpectingMore) => {
                    self.prime_outbox();
                }
                _chain => {
                    error!(
                        "unexpectedly in receiving state and got chain: {:?}",
                        _chain
                    );
                    self.reset_state();
                }
            },
        }
    }

    pub fn send_wait_extension(&mut self) -> bool {
        if self.state == State::Processing {
            // Need to send a wait extension request.
            let mut packet = RawPacket::zeroed_until(CCID_HEADER_LEN);
            packet[0] = 0x80;
            packet[6] = self.seq;

            // CCID_Rev110 6.2-3: Time Extension is requested
            packet[7] = 2 << 6;
            // Perhaps 1 is an ok multiplier?
            packet[8] = 0x1;
            self.send_packet_assuming_possible(packet);

            // Indicate we should check back again for another possible wait extension
            true
        } else {
            // No longer processing, so the reply has been sent, and we no longer need more time.
            false
        }
    }

    /// Turns false on read.  Intended for checking to see if a wait extension request needs to be started.
    pub fn did_start_processing(&mut self) -> bool {
        if self.started_processing {
            self.started_processing = false;
            true
        } else {
            false
        }
    }

    #[inline(never)]
    fn call_app(&mut self) {
        self.interchange
            .send_request()
            .expect("could not deposit command");
        self.started_processing = true;
        self.state = State::Processing;
    }

    #[inline(never)]
    pub fn poll_app(&mut self) {
        if State::Processing == self.state {
            // info!("processing, checking for response, interchange state {:?}",
            //           self.interchange.state()).ok();

            if interchange::State::Responded == self.interchange.state() {
                // we should have an open XfrBlock allowance
                self.state = State::ReadyToSend;
                self.sent = 0;
                self.prime_outbox();
            }
        }
    }

    pub fn prime_outbox(&mut self) {
        if self.state != State::ReadyToSend && self.state != State::Sending {
            return;
        }

        if self.outbox.is_some() {
            error!("Full outbox");
            self.reset_state();
            return;
        }

        let Ok(message) = self.interchange.response()  else {
            error!("Got no response while priming outbox");
            self.reset_state();
            return;
        };

        let chunk_size = core::cmp::min(PACKET_SIZE - CCID_HEADER_LEN, message.len() - self.sent);
        let chunk = &message[self.sent..][..chunk_size];
        self.sent += chunk_size;
        let more = self.sent < message.len();

        let chain = match (self.state, more) {
            (State::ReadyToSend, true) => {
                self.state = State::Sending;
                Chain::Begins
            }
            (State::ReadyToSend, false) => {
                self.state = State::Idle;
                Chain::BeginsAndEnds
            }
            (State::Sending, true) => Chain::Continues,
            (State::Sending, false) => {
                self.state = State::Idle;
                Chain::Ends
            }
            // logically impossible
            _ => {
                return;
            }
        };

        let primed_packet = DataBlock::new(self.seq, chain, chunk);
        // info!("priming {:?}", &primed_packet).ok();
        self.outbox = Some(primed_packet.into());

        // fast-lane response attempt
        self.maybe_send_packet();
    }

    fn send_empty_datablock(&mut self, chain: Chain) {
        let packet = DataBlock::new(self.seq, chain, &[]).into();
        self.send_packet_assuming_possible(packet);
    }

    fn send_slot_status_ok(&mut self) {
        let mut packet = RawPacket::zeroed_until(CCID_HEADER_LEN);
        packet[0] = 0x81;
        packet[6] = self.seq;
        self.send_packet_assuming_possible(packet);
    }

    fn send_slot_status_error(&mut self, error: Error) {
        let mut packet = RawPacket::zeroed_until(CCID_HEADER_LEN);
        packet[0] = 0x6c;
        packet[6] = self.seq;
        packet[7] = 1 << 6;
        packet[8] = error as u8;
        self.send_packet_assuming_possible(packet);
    }

    fn send_parameters(&mut self) {
        let mut packet = RawPacket::zeroed_until(17);
        packet[0] = 0x82;
        packet[1] = 7;
        packet[6] = self.seq;
        packet[9] = 1; // T=1

        // just picking the fastest values.
        //              Fi = 1Mz    Di=1
        packet[10] = (0b0001 << 4) | (0b0001);

        // just taking default value from spec.
        packet[11] = 0x10;
        // not sure, taking default.
        packet[13] = 0x15;
        // set max waiting time
        packet[15] = 0xfe;
        self.send_packet_assuming_possible(packet);
    }

    fn send_atr(&mut self) {
        let atr = self.atr.clone();
        let packet = DataBlock::new(
            self.seq,
            Chain::BeginsAndEnds,
            &atr,
            // T=0, T=1, command chaining/extended Lc+Le/no logical channels, card issuer's data "Solo 2"
            // 3B 8C 80 01 80 73 C0 21 C0 56 53 6F 6C 6F 20 32 A4
            // https://smartcard-atr.apdu.fr/parse?ATR=3B+8C+80+01+80+73+C0+21+C0+56+53+6F+6C+6F+20+32+A4
            // &[0x3B, 0x8C, 0x80, 0x01, 0x80, 0x73, 0xC0, 0x21, 0xC0, 0x56, 0x53, 0x6F, 0x6C, 0x6F, 0x20, 0x32, 0xA4]
            //
            // Not sure if we also need some TA/TB/TC data as in
            // https://smartcard-atr.apdu.fr/parse?ATR=3B+F8+13+00+00+81+31+FE+15+59+75+62+69+6B+65+79+34+D4
            // At least TB(1) is deprecated, so it makes no sense
            // Also, there TD(1) = 0x81 and TD(2) = 0x31 both refer to protocol T=1 which seems wrong
        );
        self.send_packet_assuming_possible(packet.into());
    }

    fn send_packet_assuming_possible(&mut self, packet: RawPacket) {
        if self.outbox.is_some() {
            // Previous transaction will fail, but we'll be ready for new transactions.
            self.state = State::Idle;
            info!("overwriting last session..");
        }
        self.outbox = Some(packet);

        // fast-lane response attempt
        self.maybe_send_packet();
    }

    #[inline(never)]
    pub fn maybe_send_packet(&mut self) {
        if let Some(packet) = self.outbox.as_ref() {
            let needs_zlp = packet.len() == PACKET_SIZE;
            match self.write.write(packet) {
                Ok(n) if n == packet.len() => {
                    // if packet.len() > 8 {
                    //     info!("--> sent {:?}... successfully", &packet[..8]).ok();
                    // } else {
                    //     info!("--> sent {:?} successfully", packet).ok();
                    // }

                    if needs_zlp {
                        self.outbox = Some(RawPacket::new());
                    } else {
                        self.outbox = None;
                    }
                }
                Ok(_sent) => {
                    error!("Failed to send entire packet, sent only {}", _sent);
                    self.reset_state()
                }

                Err(UsbError::WouldBlock) => {
                    // fine, can't write try later
                    // this shouldn't happen probably
                    info!("waiting to send");
                }

                Err(_err) => {
                    error!("Failed to send packet {:?}", _err);
                    self.reset_state()
                }
            }
        }
    }

    // pub fn read_address(&self) -> EndpointAddress {
    //     self.read.address()
    // }

    // pub fn write_address(&self) -> EndpointAddress {
    //     self.write.address()
    // }

    // Called if we receive an ABORT request on the control pipe.
    pub fn expect_abort(&mut self, slot: u8, seq: u8) {
        debug_assert!(slot == 0);
        info!("ABORT expected for seq = {}", seq);
        // We only have one slot (see FUNCTIONAL_INTERFACE_DESCRIPTOR in constants.rs)
        if slot != 0 {
            return;
        }
        if self.bulk_abort == Some(seq) {
            self.abort();
        } else {
            self.control_abort = Some(seq);
        }
    }

    // This method performs an abort and should only be called if we received matching ABORT
    // requets both from the control pipe and from the bulk endpoint.
    fn abort(&mut self) {
        // reset state
        self.bulk_abort = None;
        self.control_abort = None;
        self.state = State::Idle;
        self.outbox = None;
        self.started_processing = false;
        self.receiving_long = false;
        self.long_packet_missing = 0;
        self.interchange.cancel().ok();

        // send response for successful abort
        self.send_slot_status_ok();
    }
}
