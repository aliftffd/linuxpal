const BUBBLE_TICKS: u64 = 80; // 8 seconds at 10Hz

pub struct Bubble {
    pub tip:     String,
    pub joke:    String,
    pub visible: bool,
    ticks_left:  u64,
}

impl Bubble {
    pub fn new() -> Self {
        Self {
            tip:        String::new(),
            joke:       String::new(),
            visible:    false,
            ticks_left: 0,
        }
    }

    /// Show immediately with loading placeholders
    pub fn show_loading(&mut self) {
        self.tip        = "thinking...".into();
        self.joke       = "thinking...".into();
        self.visible    = true;
        self.ticks_left = BUBBLE_TICKS;
    }

    /// Show a one-off remark (used for screen-edge comments).
    pub fn say(&mut self, msg: &str) {
        self.tip = msg.to_string();
        self.joke = String::new();
        self.visible = true;
        self.ticks_left = BUBBLE_TICKS;
    }

    /// Update content when LLM responds
    pub fn update(&mut self, tip: &str, joke: &str) {
        self.tip        = tip.to_string();
        self.joke       = joke.to_string();
        // reset timer so user has full time to read
        self.ticks_left = BUBBLE_TICKS;
    }

    pub fn tick(&mut self) {
        if self.visible {
            if self.ticks_left == 0 {
                self.visible = false;
            } else {
                self.ticks_left -= 1;
            }
        }
    }
}

const TAIL_LEN: usize = 8;
const TAIL_GAP: usize = 6;

/// Draw two-section bubble: TIP on top, JOKE on bottom.
/// Box hugs mascot's left edge so tail points right toward it.
/// `mascot_left` = canvas x where mascot sprite begins.
pub fn draw_bubble(
    buf:    &mut [u8],
    stride: usize,
    tip:    &str,
    joke:   &str,
    mascot_left: usize,
) {
    let bw = 258usize;
    let bh = 100usize;
    let by = 8usize;
    // anchor box right edge so tail tip lands just left of mascot
    let box_right = mascot_left.saturating_sub(TAIL_LEN + TAIL_GAP);
    let bx = box_right.saturating_sub(bw).max(4);

    // background — dark navy
    fill_rect(buf, stride, bx, by, bw, bh, 0x14, 0x1e, 0x2a, 220);

    // border — teal
    draw_border(buf, stride, bx, by, bw, bh, 0x2a, 0x5a, 0x50);

    // tail pointing right toward mascot
    for i in 0..TAIL_LEN {
        let tx = bx + bw + i;
        let ty = by + bh / 2 - (7 - i);
        let th = (i * 2 + 2).min(bh);
        fill_rect(buf, stride, tx, ty, 1, th, 0x14, 0x1e, 0x2a, 220);
    }

    // TIP label — amber #e8943c
    draw_text(buf, stride, "TIP", bx + 10, by + 10, 0xe8, 0x94, 0x3c);

    // tip content — cream
    let tip_lines = wrap_text(tip, 28);
    for (i, line) in tip_lines.iter().take(2).enumerate() {
        draw_text(buf, stride, line, bx + 10, by + 22 + i * 13, 0xe0, 0xd8, 0x98);
    }

    // divider
    fill_rect(buf, stride, bx + 8, by + 52, bw - 16, 1, 0x2a, 0x5a, 0x50, 200);

    // JOKE label — purple #9060c0
    draw_text(buf, stride, "JOKE", bx + 10, by + 58, 0x90, 0x60, 0xc0);

    // joke content — cream
    let joke_lines = wrap_text(joke, 28);
    for (i, line) in joke_lines.iter().take(2).enumerate() {
        draw_text(buf, stride, line, bx + 10, by + 70 + i * 13, 0xe0, 0xd8, 0x98);
    }
}

fn wrap_text(text: &str, max_chars: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if current.len() + word.len() + 1 > max_chars && !current.is_empty() {
            lines.push(current.clone());
            current = word.to_string();
        } else {
            if !current.is_empty() { current.push(' '); }
            current.push_str(word);
        }
    }
    if !current.is_empty() { lines.push(current); }
    lines
}

fn fill_rect(
    buf: &mut [u8], stride: usize,
    x: usize, y: usize, w: usize, h: usize,
    r: u8, g: u8, b: u8, a: u8,
) {
    for row in y..y + h {
        for col in x..x + w {
            let off = row * stride + col * 4;
            if off + 3 >= buf.len() { continue; }
            buf[off]     = b;
            buf[off + 1] = g;
            buf[off + 2] = r;
            buf[off + 3] = a;
        }
    }
}

fn draw_border(
    buf: &mut [u8], stride: usize,
    x: usize, y: usize, w: usize, h: usize,
    r: u8, g: u8, b: u8,
) {
    fill_rect(buf, stride, x,     y,       w, 2, r, g, b, 255);
    fill_rect(buf, stride, x,     y + h - 2, w, 2, r, g, b, 255);
    fill_rect(buf, stride, x,     y,       2, h, r, g, b, 255);
    fill_rect(buf, stride, x + w - 2, y,   2, h, r, g, b, 255);
}

fn draw_text(
    buf: &mut [u8], stride: usize,
    text: &str, x: usize, y: usize,
    r: u8, g: u8, b: u8,
) {
    let mut cx = x;
    for ch in text.chars() {
        let glyph = get_glyph(ch);
        for (row, &bits) in glyph.iter().enumerate() {
            for col in 0..5usize {
                if bits & (1 << (4 - col)) != 0 {
                    let off = (y + row) * stride + (cx + col) * 4;
                    if off + 3 >= buf.len() { continue; }
                    buf[off]     = b;
                    buf[off + 1] = g;
                    buf[off + 2] = r;
                    buf[off + 3] = 255;
                }
            }
        }
        cx += 6;
    }
}

fn get_glyph(c: char) -> [u8; 7] {
    match c {
        'a' => [0b00000,0b01110,0b00001,0b01111,0b10001,0b10011,0b01101],
        'b' => [0b10000,0b10000,0b11110,0b10001,0b10001,0b10001,0b11110],
        'c' => [0b00000,0b01110,0b10000,0b10000,0b10000,0b10001,0b01110],
        'd' => [0b00001,0b00001,0b01111,0b10001,0b10001,0b10001,0b01111],
        'e' => [0b00000,0b01110,0b10001,0b11111,0b10000,0b10001,0b01110],
        'f' => [0b00110,0b01001,0b01000,0b11100,0b01000,0b01000,0b01000],
        'g' => [0b00000,0b01111,0b10001,0b10001,0b01111,0b00001,0b01110],
        'h' => [0b10000,0b10000,0b11110,0b10001,0b10001,0b10001,0b10001],
        'i' => [0b00100,0b00000,0b01100,0b00100,0b00100,0b00100,0b01110],
        'j' => [0b00010,0b00000,0b00110,0b00010,0b00010,0b10010,0b01100],
        'k' => [0b10000,0b10010,0b10100,0b11000,0b10100,0b10010,0b10001],
        'l' => [0b01100,0b00100,0b00100,0b00100,0b00100,0b00100,0b01110],
        'm' => [0b00000,0b11010,0b10101,0b10101,0b10001,0b10001,0b10001],
        'n' => [0b00000,0b11110,0b10001,0b10001,0b10001,0b10001,0b10001],
        'o' => [0b00000,0b01110,0b10001,0b10001,0b10001,0b10001,0b01110],
        'p' => [0b00000,0b11110,0b10001,0b10001,0b11110,0b10000,0b10000],
        'q' => [0b00000,0b01110,0b10001,0b10001,0b01111,0b00001,0b00001],
        'r' => [0b00000,0b10110,0b11001,0b10000,0b10000,0b10000,0b10000],
        's' => [0b00000,0b01111,0b10000,0b01110,0b00001,0b00001,0b11110],
        't' => [0b01000,0b01000,0b11110,0b01000,0b01000,0b01001,0b00110],
        'u' => [0b00000,0b10001,0b10001,0b10001,0b10001,0b10011,0b01101],
        'v' => [0b00000,0b10001,0b10001,0b10001,0b10001,0b01010,0b00100],
        'w' => [0b00000,0b10001,0b10001,0b10101,0b10101,0b10101,0b01010],
        'x' => [0b00000,0b10001,0b01010,0b00100,0b01010,0b10001,0b00000],
        'y' => [0b00000,0b10001,0b10001,0b01111,0b00001,0b10001,0b01110],
        'z' => [0b00000,0b11111,0b00010,0b00100,0b01000,0b10000,0b11111],
        'A' => [0b00100,0b01010,0b10001,0b11111,0b10001,0b10001,0b10001],
        'B' => [0b11110,0b10001,0b10001,0b11110,0b10001,0b10001,0b11110],
        'C' => [0b01110,0b10001,0b10000,0b10000,0b10000,0b10001,0b01110],
        'D' => [0b11110,0b10001,0b10001,0b10001,0b10001,0b10001,0b11110],
        'E' => [0b11111,0b10000,0b10000,0b11110,0b10000,0b10000,0b11111],
        'F' => [0b11111,0b10000,0b10000,0b11110,0b10000,0b10000,0b10000],
        'G' => [0b01110,0b10001,0b10000,0b10111,0b10001,0b10001,0b01110],
        'H' => [0b10001,0b10001,0b10001,0b11111,0b10001,0b10001,0b10001],
        'I' => [0b01110,0b00100,0b00100,0b00100,0b00100,0b00100,0b01110],
        'J' => [0b00111,0b00010,0b00010,0b00010,0b00010,0b10010,0b01100],
        'K' => [0b10001,0b10010,0b10100,0b11000,0b10100,0b10010,0b10001],
        'L' => [0b10000,0b10000,0b10000,0b10000,0b10000,0b10000,0b11111],
        'M' => [0b10001,0b11011,0b10101,0b10001,0b10001,0b10001,0b10001],
        'N' => [0b10001,0b11001,0b10101,0b10011,0b10001,0b10001,0b10001],
        'O' => [0b01110,0b10001,0b10001,0b10001,0b10001,0b10001,0b01110],
        'P' => [0b11110,0b10001,0b10001,0b11110,0b10000,0b10000,0b10000],
        'Q' => [0b01110,0b10001,0b10001,0b10001,0b10101,0b10010,0b01101],
        'R' => [0b11110,0b10001,0b10001,0b11110,0b10100,0b10010,0b10001],
        'S' => [0b01111,0b10000,0b10000,0b01110,0b00001,0b00001,0b11110],
        'T' => [0b11111,0b00100,0b00100,0b00100,0b00100,0b00100,0b00100],
        'U' => [0b10001,0b10001,0b10001,0b10001,0b10001,0b10001,0b01110],
        'V' => [0b10001,0b10001,0b10001,0b10001,0b10001,0b01010,0b00100],
        'W' => [0b10001,0b10001,0b10001,0b10101,0b10101,0b11011,0b10001],
        'X' => [0b10001,0b01010,0b00100,0b00100,0b00100,0b01010,0b10001],
        'Y' => [0b10001,0b10001,0b01010,0b00100,0b00100,0b00100,0b00100],
        'Z' => [0b11111,0b00001,0b00010,0b00100,0b01000,0b10000,0b11111],
        '0' => [0b01110,0b10011,0b10101,0b10101,0b10101,0b11001,0b01110],
        '1' => [0b00100,0b01100,0b00100,0b00100,0b00100,0b00100,0b01110],
        '2' => [0b01110,0b10001,0b00001,0b00110,0b01000,0b10000,0b11111],
        '3' => [0b11111,0b00001,0b00010,0b00110,0b00001,0b10001,0b01110],
        '4' => [0b00010,0b00110,0b01010,0b10010,0b11111,0b00010,0b00010],
        '5' => [0b11111,0b10000,0b11110,0b00001,0b00001,0b10001,0b01110],
        '6' => [0b00110,0b01000,0b10000,0b11110,0b10001,0b10001,0b01110],
        '7' => [0b11111,0b00001,0b00010,0b00100,0b01000,0b01000,0b01000],
        '8' => [0b01110,0b10001,0b10001,0b01110,0b10001,0b10001,0b01110],
        '9' => [0b01110,0b10001,0b10001,0b01111,0b00001,0b00010,0b01100],
        ':' => [0b00000,0b00100,0b00100,0b00000,0b00100,0b00100,0b00000],
        '-' => [0b00000,0b00000,0b00000,0b01110,0b00000,0b00000,0b00000],
        '+' => [0b00000,0b00100,0b00100,0b11111,0b00100,0b00100,0b00000],
        '/' => [0b00001,0b00010,0b00010,0b00100,0b01000,0b01000,0b10000],
        '.' => [0b00000,0b00000,0b00000,0b00000,0b00000,0b00110,0b00110],
        ',' => [0b00000,0b00000,0b00000,0b00000,0b00110,0b00100,0b01000],
        '!' => [0b00100,0b00100,0b00100,0b00100,0b00000,0b00000,0b00100],
        '?' => [0b01110,0b10001,0b00001,0b00110,0b00100,0b00000,0b00100],
        '<' => [0b00010,0b00100,0b01000,0b10000,0b01000,0b00100,0b00010],
        '>' => [0b01000,0b00100,0b00010,0b00001,0b00010,0b00100,0b01000],
        '_' => [0b00000,0b00000,0b00000,0b00000,0b00000,0b00000,0b11111],
        ' ' => [0b00000,0b00000,0b00000,0b00000,0b00000,0b00000,0b00000],
        _   => [0b11111,0b10001,0b10001,0b10001,0b10001,0b10001,0b11111],
    }
}
