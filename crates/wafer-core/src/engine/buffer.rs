//! Host-side buffer resource for `borrow<buffer>` in WIT contracts.
//!
//! `WaferBuffer` wraps `Bytes` and lives in the per-Store `ResourceTable`.
//! One buffer handle is created per `process()`/`evaluate()`/`route()` call
//! and destroyed immediately after — never held across message boundaries.
//!
//! See docs/rfcs/RFC-002-host-runtime.md D2.

use bytes::Bytes;

/// Host-side implementation of the WIT `resource buffer`.
///
/// Wraps a `Bytes` handle to the message payload. Since `Bytes::clone()` is
/// a refcount bump (~5ns), creating a `WaferBuffer` from a `RuntimeEnvelope`
/// involves no data copy.
pub struct WaferBuffer {
    data: Bytes,
}

impl WaferBuffer {
    /// Create a buffer from existing payload bytes.
    ///
    /// # Performance
    /// `Bytes::clone()` is an Arc refcount bump — no allocation or copy.
    #[inline]
    pub const fn new(data: Bytes) -> Self {
        Self { data }
    }

    /// Total byte length of the payload.
    #[inline]
    pub const fn size(&self) -> u64 {
        self.data.len() as u64
    }

    /// Read a slice of the payload. Returns fewer bytes if offset+len exceeds size.
    #[inline]
    pub fn read(&self, offset: u64, len: u64) -> Vec<u8> {
        let total = self.data.len() as u64;
        if offset >= total {
            return Vec::new();
        }
        let remaining = total - offset;
        let clamped_len = len.min(remaining) as usize;
        let start = offset as usize;
        self.data[start..start + clamped_len].to_vec()
    }

    /// Read the entire payload in one call.
    #[inline]
    pub fn read_all(&self) -> Vec<u8> {
        self.data.to_vec()
    }

    /// Borrow the underlying bytes without copying.
    #[inline]
    pub const fn as_bytes(&self) -> &Bytes {
        &self.data
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_buffer() {
        let buf = WaferBuffer::new(Bytes::new());
        assert_eq!(buf.size(), 0);
        assert!(buf.read_all().is_empty());
        assert!(buf.read(0, 10).is_empty());
    }

    #[test]
    fn buffer_size() {
        let data = Bytes::from_static(b"hello world");
        let buf = WaferBuffer::new(data);
        assert_eq!(buf.size(), 11);
    }

    #[test]
    fn read_all() {
        let data = Bytes::from_static(b"hello");
        let buf = WaferBuffer::new(data);
        assert_eq!(buf.read_all(), b"hello");
    }

    #[test]
    fn read_slice() {
        let data = Bytes::from_static(b"hello world");
        let buf = WaferBuffer::new(data);
        assert_eq!(buf.read(0, 5), b"hello");
        assert_eq!(buf.read(6, 5), b"world");
    }

    #[test]
    fn read_clamped_to_size() {
        let data = Bytes::from_static(b"hi");
        let buf = WaferBuffer::new(data);
        // Requesting more than available: returns what's there
        assert_eq!(buf.read(0, 100), b"hi");
        // Offset beyond end: returns empty
        assert!(buf.read(100, 10).is_empty());
    }

    #[test]
    fn read_partial_offset() {
        let data = Bytes::from_static(b"abcdefgh");
        let buf = WaferBuffer::new(data);
        // Read from offset 5, requesting 10 bytes — only 3 available
        assert_eq!(buf.read(5, 10), b"fgh");
    }

    #[test]
    fn clone_is_cheap() {
        let data = Bytes::from(vec![0u8; 1024]);
        let buf = WaferBuffer::new(data.clone());
        // Verify the underlying Bytes share the same allocation
        assert_eq!(buf.as_bytes(), &data);
    }
}
