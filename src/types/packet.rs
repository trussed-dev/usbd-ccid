use core::{
    convert::{TryFrom, TryInto},
    ops::{Deref, DerefMut},
};

use crate::constants::*;

pub type RawPacket = heapless::Vec<u8, PACKET_SIZE>;

#[derive(Default, PartialEq, Eq)]
pub struct ExtPacket(heapless::Vec<u8, MAX_MSG_LENGTH>);

pub trait RawPacketExt {
    fn data_len(&self) -> usize;
}

impl RawPacketExt for RawPacket {
    fn data_len(&self) -> usize {
        u32::from_le_bytes(self[1..5].try_into().unwrap()) as usize
    }
}

pub enum Error {
    ShortPacket,
    UnknownCommand(u8),
}

pub trait Packet: core::ops::Deref<Target = heapless::Vec<u8, MAX_MSG_LENGTH>> {
    #[inline]
    fn slot(&self) -> u8 {
        // we have only one slot
        assert!(self[5] == 0);
        self[5]
    }

    #[inline]
    fn seq(&self) -> u8 {
        self[6]
    }
}

pub trait PacketWithData: Packet {
    #[inline]
    fn data(&self) -> &[u8] {
        // let len = u32::from_le_bytes(self[1..5].try_into().unwrap()) as usize;
        let declared_len = u32::from_le_bytes(self[1..5].try_into().unwrap()) as usize;
        let len = core::cmp::min(MAX_MSG_LENGTH - CCID_HEADER_LEN, declared_len);
        // hprintln!("delcared = {}, len = {}", declared_len, len).ok();
        &self[CCID_HEADER_LEN..][..len]
    }
}

pub trait ChainedPacket: Packet {
    #[inline(always)]
    fn chain(&self) -> Chain {
        let level_parameter = u16::from_le_bytes(self[8..10].try_into().unwrap());
        match level_parameter {
            0 => Chain::BeginsAndEnds,
            1 => Chain::Begins,
            2 => Chain::Ends,
            3 => Chain::Continues,
            0x10 => Chain::ExpectingMore,
            _ => panic!("invalid power select parameter"),
        }
    }
}

impl Deref for ExtPacket {
    type Target = heapless::Vec<u8, MAX_MSG_LENGTH>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for ExtPacket {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Packet for ExtPacket {}
impl PacketWithData for ExtPacket {}
impl ChainedPacket for ExtPacket {}

pub struct DataBlock<'a> {
    seq: u8,
    chain: Chain,
    data: &'a [u8],
}

impl<'a> DataBlock<'a> {
    pub fn new(seq: u8, chain: Chain, data: &'a [u8]) -> Self {
        assert!(data.len() + CCID_HEADER_LEN <= PACKET_SIZE);
        Self { seq, chain, data }
    }
}

impl core::fmt::Debug for DataBlock<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut debug_struct = f.debug_struct("DataBlock");

        debug_struct.field("seq", &self.seq);

        let l = core::cmp::min(self.data.len(), 16);
        let escaped_bytes: heapless::Vec<u8, 64> = self
            .data
            .iter()
            .take(l)
            .flat_map(|byte| core::ascii::escape_default(*byte))
            .collect();
        let data_as_str = &core::str::from_utf8(&escaped_bytes).unwrap();

        debug_struct
            .field("chain", &self.chain)
            .field("len", &self.data.len())
            .field("data", &format_args!("b'{data_as_str}'"))
            .finish()
    }
}

// WELL. DataBlock does not deref to RawPacket
// impl Deref for DataBlock<_> {
//     type Target: &

// impl Packet for DataBlock<'_> {
//     fn slot(&self) -> u8 { 0 }
//     fn seq(&self) -> u8 { self.seq }
// }

impl From<DataBlock<'_>> for RawPacket {
    fn from(block: DataBlock<'_>) -> RawPacket {
        let mut packet = RawPacket::new();
        let len = block.data.len();
        packet.resize_default(CCID_HEADER_LEN + len).ok();
        packet[0] = 0x80;
        packet[1..][..4].copy_from_slice(
            &u32::try_from(len)
                .expect("Packets should not be more than 4GiB")
                .to_le_bytes(),
        );
        packet[5] = 0;
        packet[6] = block.seq;

        // status
        packet[7] = 0;
        // error
        packet[8] = 0;
        // chain parameter
        packet[9] = block.chain as u8;
        packet[CCID_HEADER_LEN..][..len].copy_from_slice(block.data);

        packet
    }
}

#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CommandKind {
    // REQUESTS

    // supported
    PowerOn = 0x62,
    PowerOff = 0x63,
    GetSlotStatus = 0x65,
    GetParameters = 0x6c,
    XfrBlock = 0x6f,
    Abort = 0x72,
    // unsupported
    // ResetParameters = 0x6d,
    // SetParameters = 0x61,
    // Escape = 0x6b, //  for vendor commands
    // IccClock = 0x7e,
    // T0Apdu = 0x6a,
    // Secure = 0x69,
    // Mechanical = 0x71,
    // SetDataRateAndClockFrequency = 0x73,
}

impl ExtPacket {
    pub fn command_type(&self) -> Result<CommandKind, Error> {
        if self.len() < CCID_HEADER_LEN {
            return Err(Error::ShortPacket);
        }
        if self[5] != 0 {
            // wrong slot
        }
        let command_byte = self[0];
        match command_byte {
            0x62 => Ok(CommandKind::PowerOn),
            0x63 => Ok(CommandKind::PowerOff),
            0x65 => Ok(CommandKind::GetSlotStatus),
            0x6c => Ok(CommandKind::GetParameters),
            0x6f => Ok(CommandKind::XfrBlock),
            0x72 => Ok(CommandKind::Abort),
            _ => Err(Error::UnknownCommand(command_byte)),
        }
    }
}
// command_message!(
//     PowerOn: 0x62,
//     PowerOff: 0x63,
//     GetSlotStatus: 0x65,
//     GetParameters: 0x6c,
//     XfrBlock: 0x6f,
//     Abort: 0x72,
// );

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Chain {
    BeginsAndEnds = 0,
    Begins = 1,
    Ends = 2,
    Continues = 3,
    ExpectingMore = 0x10,
}

impl Chain {
    pub fn transfer_ongoing(&self) -> bool {
        matches!(
            self,
            Chain::BeginsAndEnds | Chain::Ends | Chain::ExpectingMore
        )
    }
}

impl core::fmt::Debug for ExtPacket {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut debug_struct = f.debug_struct("Command");
        // write!("Command({:?})", &self.command_type()));
        // // "Command");

        let Ok(command_type) = self.command_type() else {
            return debug_struct.field("cmd", &format_args!("error")).field("value", &format_args!("{:02x?}", self.0)).finish();
        };
        debug_struct
            .field("cmd", &command_type)
            .field("seq", &self.seq());

        if command_type == CommandKind::XfrBlock {
            let l = core::cmp::min(self.len(), 8);
            let escaped_bytes: heapless::Vec<u8, 64> = self
                .data()
                .iter()
                .take(l)
                .flat_map(|byte| core::ascii::escape_default(*byte))
                .collect();
            let data_as_str = &core::str::from_utf8(&escaped_bytes).unwrap();

            debug_struct
                .field("chain", &self.chain())
                .field("len", &self.data().len());

            if l < self.len() {
                debug_struct.field("data[..8]", &format_args!("b'{data_as_str}'"))
            } else {
                debug_struct.field("data", &format_args!("b'{data_as_str}'"))
            };
        }

        // let mut debug_struct = match self.msg_type() {
        //     Ok(message_type) => debug_struct.field("type", &message_type),
        //     Err(()) => debug_struct.field("type", &self[0]),
        // };

        // let has_data = self.len() > 0;
        debug_struct.finish()
    }
}
