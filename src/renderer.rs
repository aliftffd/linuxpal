use crate::sprites::Frame;

pub fn blit_frame(buf: &mut [u8], buf_width: u32, buf_height: u32, stride: usize, frame: &Frame, x_offset: usize) {
    let fw = frame.width as usize;
    let fh = frame.height as usize;

    for row in 0..fh.min(buf_height as usize) {
        for col in 0..fw.min(buf_width as usize) {
            let src = (row * fw + col) * 4;
            let dst = row * stride + (col + x_offset) * 4;

            if src + 3 >= frame.data.len() || dst + 3 >= buf.len() {
                continue;
            }

            let r = frame.data[src];
            let g = frame.data[src + 1];
            let b = frame.data[src + 2];
            let a = frame.data[src + 3];

            // Shm format Argb8888 is actually BGRA in little-endian (or BGRX)
            // But we need to handle pre-multiplied alpha if compositor expects it.
            // SCTK SlotPool usually expects pre-multiplied.
            
            let alpha = a as f32 / 255.0;
            buf[dst] = (b as f32 * alpha) as u8;
            buf[dst + 1] = (g as f32 * alpha) as u8;
            buf[dst + 2] = (r as f32 * alpha) as u8;
            buf[dst + 3] = a;
        }
    }
}

pub fn clear(buf: &mut [u8]) {
    buf.fill(0);
}
