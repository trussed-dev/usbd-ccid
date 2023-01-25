use core::convert::{TryFrom, TryInto};

use crate::constants::*;

pub type RawPacket = heapless::Vec<u8, PACKET_SIZE>;
pub type ExtPacket = heapless::Vec<u8, MAX_MSG_LENGTH>;

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

pub trait Packet: core::ops::Deref<Target = ExtPacket> {
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

impl ChainedPacket for XfrBlock {}

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
            .field("data", &format_args!("b'{}'", data_as_str))
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
#[derive(Copy, Clone, Debug)]
pub enum CommandType {
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

macro_rules! command_message {

    ($($Name:ident: $code:expr,)*) => {
        $(
            #[derive(Debug)]
            pub struct $Name {
                // use reference? pulls in lifetimes though...
                ext_raw: ExtPacket,
            }

            impl core::ops::Deref for $Name {
                type Target = ExtPacket;

                #[inline]
                fn deref(&self) -> &Self::Target {
                    &self.ext_raw
                }
            }

            impl core::ops::DerefMut for $Name {

                #[inline]
                fn deref_mut(&mut self) -> &mut Self::Target {
                    &mut self.ext_raw
                }
            }

            impl Packet for $Name {}
        )*

        pub enum Command {
            $(
                $Name($Name),
            )*
        }

        impl Command {
            pub fn seq(&self) -> u8 {
                match self {
                    $(
                        Command::$Name(packet) => packet.seq(),
                    )*
                }
            }

            pub fn command_type(&self) -> CommandType {
                match self {
                    $(
                        Command::$Name(_) => CommandType::$Name,
                    )*
                }
            }
        }

        impl core::convert::TryFrom<ExtPacket> for Command {
            type Error = Error;

            #[inline]
            fn try_from(packet: ExtPacket)
                -> core::result::Result<Self, Self::Error>
            {
                if packet.len() <CCID_HEADER_LEN {
                    return Err(Error::ShortPacket);
                }
                if packet[5] != 0 {
                    // wrong slot
                }
                let command_byte = packet[0];
                Ok(match command_byte {
                    $(
                        $code => Command::$Name($Name { ext_raw: packet } ),
                    )*
                    _ => return Err(Error::UnknownCommand(command_byte)),
                })
            }
        }

        impl core::ops::Deref for Command {
            type Target = ExtPacket;

            #[inline]
            fn deref(&self) -> &Self::Target {
                match self {
                    $(
                        Command::$Name(packet) => &packet,
                    )*
                }
            }
        }

        // impl core::ops::DerefMut for Command {

        //     #[inline]
        //     fn deref_mut(&mut self) -> &mut Self::Target {
        //         match self {
        //             $(
        //                 Command::$Name(packet) => &mut packet,
        //             )*
        //         }
        //     }
        // }
    }
}

command_message!(
    PowerOn: 0x62,
    PowerOff: 0x63,
    GetSlotStatus: 0x65,
    GetParameters: 0x6c,
    XfrBlock: 0x6f,
    Abort: 0x72,
);

impl PacketWithData for XfrBlock {}

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

impl core::fmt::Debug for Command {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut debug_struct = f.debug_struct("Command");
        // write!("Command({:?})", &self.command_type()));
        // // "Command");

        debug_struct
            .field("cmd", &self.command_type())
            .field("seq", &self.seq());

        if let Command::XfrBlock(block) = self {
            let l = core::cmp::min(self.len(), 8);
            let escaped_bytes: heapless::Vec<u8, 64> = block
                .data()
                .iter()
                .take(l)
                .flat_map(|byte| core::ascii::escape_default(*byte))
                .collect();
            let data_as_str = &core::str::from_utf8(&escaped_bytes).unwrap();

            debug_struct
                .field("chain", &block.chain())
                .field("len", &block.data().len());

            if l < self.len() {
                debug_struct.field("data[..8]", &format_args!("b'{}'", data_as_str))
            } else {
                debug_struct.field("data", &format_args!("b'{}'", data_as_str))
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
