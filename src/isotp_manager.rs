use crate::ble_protocol::*;
use crate::isotp_handler::IsotpHandler;
use heapless::FnvIndexMap;

const MAX_HANDLERS: usize = 6;

pub struct IsoTpManager {
    handlers: FnvIndexMap<u32, IsotpHandler, MAX_HANDLERS>,
}

impl IsoTpManager {
    pub fn new() -> Self {
        Self {
            handlers: FnvIndexMap::new(),
        }
    }
}
