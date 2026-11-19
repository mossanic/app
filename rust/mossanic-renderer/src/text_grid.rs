use bytemuck::{Pod, Zeroable};
use mossanic_terminal_core::{Cell, Terminal, TerminalColor};

pub const ATLAS_WIDTH: u32 = 1024;
pub const ATLAS_HEIGHT: u32 = 8;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 2],
    pub uv: [f32; 2],
    pub foreground: [f32; 4],
    pub background: [f32; 4],
}

/// Builds six vertices per terminal cell so both the cell background and its
/// bitmap glyph are drawn in one textured quad.
pub fn build_grid_vertices(terminal: &Terminal, width: u32, height: u32) -> Vec<Vertex> {
    let mut vertices = Vec::with_capacity(terminal.columns() * terminal.rows() * 6);
    build_grid_vertices_into(terminal, width, height, &mut vertices);
    vertices
}

/// Rebuild the terminal grid into reusable storage without allocating each frame.
pub fn build_grid_vertices_into(
    terminal: &Terminal,
    width: u32,
    height: u32,
    vertices: &mut Vec<Vertex>,
) {
    let columns = terminal.columns();
    let rows = terminal.rows();
    let cell_size = (width as f32 / columns as f32).min(height as f32 / rows as f32);
    let grid_width = cell_size * columns as f32;
    let grid_height = cell_size * rows as f32;
    let offset_x = (width as f32 - grid_width) * 0.5;
    let offset_y = (height as f32 - grid_height) * 0.5;
    vertices.clear();
    vertices.reserve(columns * rows * 6);
    let (cursor_x, cursor_y) = terminal.cursor();

    for row_index in 0..rows {
        let Some(row) = terminal.row(row_index) else {
            continue;
        };
        for (column, cell) in row.iter().enumerate() {
            let mut foreground = color_rgba(
                cell.style.foreground,
                cell.style.bold,
                [0.88, 0.90, 0.94, 1.0],
            );
            let mut background = color_rgba(cell.style.background, false, [0.015, 0.02, 0.03, 1.0]);
            if cell.style.inverse
                || (terminal.cursor_visible() && column == cursor_x && row_index == cursor_y)
            {
                std::mem::swap(&mut foreground, &mut background);
            }
            let glyph = glyph_index(cell);
            let x0 = offset_x + column as f32 * cell_size;
            let y0 = offset_y + row_index as f32 * cell_size;
            append_cell(
                vertices,
                [x0, y0, x0 + cell_size, y0 + cell_size],
                glyph,
                foreground,
                background,
            );
        }
    }
}

fn glyph_index(cell: &Cell) -> usize {
    let codepoint = cell.character as u32;
    if (32..=126).contains(&codepoint) {
        (codepoint - 32) as usize
    } else {
        (b'?' - 32) as usize
    }
}

fn append_cell(
    vertices: &mut Vec<Vertex>,
    bounds: [f32; 4],
    glyph_index: usize,
    foreground: [f32; 4],
    background: [f32; 4],
) {
    let [x0, y0, x1, y1] = bounds;
    let u0 = (glyph_index as u32 * 8) as f32 / ATLAS_WIDTH as f32;
    let u1 = ((glyph_index as u32 + 1) * 8) as f32 / ATLAS_WIDTH as f32;
    let v0 = 0.0;
    let v1 = 1.0;
    let vertex = |position, uv| Vertex {
        position,
        uv,
        foreground,
        background,
    };
    vertices.extend_from_slice(&[
        vertex([x0, y0], [u0, v0]),
        vertex([x1, y0], [u1, v0]),
        vertex([x0, y1], [u0, v1]),
        vertex([x1, y0], [u1, v0]),
        vertex([x1, y1], [u1, v1]),
        vertex([x0, y1], [u0, v1]),
    ]);
}

fn color_rgba(color: TerminalColor, bold: bool, default: [f32; 4]) -> [f32; 4] {
    let (red, green, blue) = match color {
        TerminalColor::Default => return default,
        TerminalColor::Rgb(red, green, blue) => (red, green, blue),
        TerminalColor::Indexed(index) => indexed_color(index, bold),
    };
    [
        red as f32 / 255.0,
        green as f32 / 255.0,
        blue as f32 / 255.0,
        1.0,
    ]
}

fn indexed_color(index: u8, bold: bool) -> (u8, u8, u8) {
    const ANSI: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (205, 49, 49),
        (13, 188, 121),
        (229, 229, 16),
        (36, 114, 200),
        (188, 63, 188),
        (17, 168, 205),
        (229, 229, 229),
        (102, 102, 102),
        (241, 76, 76),
        (35, 209, 139),
        (245, 245, 67),
        (59, 142, 234),
        (214, 112, 214),
        (41, 184, 219),
        (255, 255, 255),
    ];

    let index = if bold && index < 8 { index + 8 } else { index };
    match index {
        0..=15 => ANSI[index as usize],
        16..=231 => {
            let cube = index - 16;
            let component = |value: u8| if value == 0 { 0 } else { 55 + value * 40 };
            (
                component(cube / 36),
                component((cube % 36) / 6),
                component(cube % 6),
            )
        }
        232..=255 => {
            let gray = 8 + (index - 232) * 10;
            (gray, gray, gray)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::build_grid_vertices;
    use bytemuck::Zeroable;
    use mossanic_terminal_core::Terminal;

    #[test]
    fn emits_two_triangles_for_every_cell() {
        let terminal = Terminal::new(3, 2);
        let vertices = build_grid_vertices(&terminal, 300, 200);
        assert_eq!(vertices.len(), 3 * 2 * 6);
        assert_eq!(vertices[0].position, [0.0, 0.0]);
        assert_eq!(vertices[1].position, [100.0, 0.0]);
        assert_eq!(vertices[5].position, [0.0, 100.0]);
    }

    #[test]
    fn centers_square_bitmap_cells_inside_a_tall_viewport() {
        let terminal = Terminal::new(3, 2);
        let vertices = build_grid_vertices(&terminal, 300, 300);
        assert_eq!(vertices[0].position, [0.0, 50.0]);
        assert_eq!(vertices[1].position, [100.0, 50.0]);
        assert_eq!(vertices[2].position, [0.0, 150.0]);
    }

    #[test]
    fn reuses_vertex_storage_and_clears_old_grid_data() {
        let terminal = Terminal::new(2, 2);
        let mut vertices = vec![super::Vertex::zeroed(); 64];
        let capacity = vertices.capacity();
        super::build_grid_vertices_into(&terminal, 80, 80, &mut vertices);
        assert_eq!(vertices.len(), 2 * 2 * 6);
        assert_eq!(vertices.capacity(), capacity);
    }

    #[test]
    fn maps_ansi_colors_and_uses_question_mark_for_unsupported_glyphs() {
        let mut terminal = Terminal::new(2, 1);
        terminal.feed("\x1b[31mλ".as_bytes());
        let vertices = build_grid_vertices(&terminal, 160, 40);
        assert_eq!(
            vertices[0].foreground,
            [205.0 / 255.0, 49.0 / 255.0, 49.0 / 255.0, 1.0]
        );
        assert_eq!(vertices[0].uv[0], ((b'?' - 32) as f32 * 8.0) / 1024.0);
    }

    #[test]
    fn inverse_style_swaps_cell_colors() {
        let mut terminal = Terminal::new(1, 1);
        terminal.feed(b"\x1b[7mA");
        let vertices = build_grid_vertices(&terminal, 80, 40);
        assert_eq!(vertices[0].foreground, [0.015, 0.02, 0.03, 1.0]);
        assert_eq!(vertices[0].background, [0.88, 0.90, 0.94, 1.0]);
    }
}
