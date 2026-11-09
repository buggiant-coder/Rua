use prost::Message as _;

include!(concat!(env!("OUT_DIR"), "/lark.ws.rs"));

impl Frame {
    pub fn header_value(&self, key: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|header| header.key == key)
            .map(|header| header.value.as_str())
    }
}

pub fn encode_frame(frame: &Frame) -> Vec<u8> {
    frame.encode_to_vec()
}

pub fn decode_frame(bytes: &[u8]) -> Result<Frame, prost::DecodeError> {
    Frame::decode(bytes)
}
