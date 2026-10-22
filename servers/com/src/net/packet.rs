use std::fmt::Debug;

use serde::{Serialize, Deserialize};

use crate::PACKET_SIZE;

pub type PacketUnit = [u8; PACKET_SIZE];
pub const PACKET: PacketUnit = [0; PACKET_SIZE];

#[derive(Clone)]
pub struct Packet {
    pub bytes: Vec<u8>,
}

impl Debug for Packet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.bytes.fmt(f)
    }
}

impl<T: Serialize> From<T> for Packet {
    fn from(value: T) -> Self {
        let mut bytes = vec![0]; // First byte is flag
        bytes.append(&mut bincode::serialize(&value).unwrap());
        Self { bytes }
    }
}

impl Packet {
    pub fn new() -> Self {
        Self { bytes: vec![] }
    }

    pub fn flags(&self) -> u8 {
        self.bytes[0]
    }

    pub fn async_process(mut self) -> Self {
        self.bytes[0] = 1;
        self
    }
    
    pub fn buildup(u0: PacketUnit) -> (Self, usize) {
        let (size, bytes) = u0.split_at(8);
        let len = usize::from_be_bytes(size.try_into().unwrap()) + 8;
        (
            Self {
                bytes: bytes.to_vec(),
            },
            if len > PACKET_SIZE {
                len - PACKET_SIZE
            } else {
                0
            },
        )
    }

    pub fn append(&mut self, u: PacketUnit, rem: usize) -> usize {
        if rem < PACKET_SIZE {
            self.bytes.append(&mut u[0..rem].to_vec());
            0
        } else {
            self.bytes.append(&mut u.to_vec());
            rem - PACKET_SIZE
        }
    }

    pub fn split(&self) -> Vec<PacketUnit> {
        let mut bytes = self.bytes.len().to_be_bytes().to_vec();
        bytes.append(&mut self.bytes.clone());
        let len = bytes.len();
        let max = if len % PACKET_SIZE == 0 {
            len / PACKET_SIZE
        } else {
            1 + len / PACKET_SIZE
        };
        bytes.resize(max * PACKET_SIZE, 0);
        let mut res = Vec::<PacketUnit>::new();
        for i in 0..max {
            let u: PacketUnit = bytes[i * PACKET_SIZE..(i + 1) * PACKET_SIZE]
                .try_into()
                .unwrap();
            res.push(u);
        }
        res
    }

    pub fn transmit(&self) -> Vec<u8> {
        let mut res = vec![];
        for slice in self.split() {
            res.append(&mut slice.to_vec())
        }
        res
    }

    pub fn unpack<'de, T>(&'de self) -> T
    where
        T: Deserialize<'de>,
    {
        let bytes = &self.bytes[1..];
        bincode::deserialize(bytes).unwrap()
    }
}
