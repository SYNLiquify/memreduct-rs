//! In-memory DLGTEMPLATEEX builder, so every window layout lives in Rust
//! code instead of a compiled .rc resource script.

#![allow(dead_code)]

// Window / dialog styles
pub const WS_POPUP: u32 = 0x8000_0000;
pub const WS_CHILD: u32 = 0x4000_0000;
pub const WS_VISIBLE: u32 = 0x1000_0000;
pub const WS_DISABLED: u32 = 0x0800_0000;
pub const WS_CLIPSIBLINGS: u32 = 0x0400_0000;
pub const WS_CLIPCHILDREN: u32 = 0x0200_0000;
pub const WS_CAPTION: u32 = 0x00C0_0000;
pub const WS_BORDER: u32 = 0x0080_0000;
pub const WS_VSCROLL: u32 = 0x0020_0000;
pub const WS_SYSMENU: u32 = 0x0008_0000;
pub const WS_GROUP: u32 = 0x0002_0000;
pub const WS_MINIMIZEBOX: u32 = 0x0002_0000;
pub const WS_TABSTOP: u32 = 0x0001_0000;

pub const WS_EX_CLIENTEDGE: u32 = 0x0000_0200;
pub const WS_EX_CONTROLPARENT: u32 = 0x0001_0000;
pub const WS_EX_APPWINDOW: u32 = 0x0004_0000;

pub const DS_SETFONT: u32 = 0x40;
pub const DS_MODALFRAME: u32 = 0x80;
pub const DS_CONTROL: u32 = 0x400;
pub const DS_CENTER: u32 = 0x800;

// Control styles
pub const BS_PUSHBUTTON: u32 = 0x0;
pub const BS_DEFPUSHBUTTON: u32 = 0x1;
pub const BS_AUTOCHECKBOX: u32 = 0x3;
pub const BS_SPLITBUTTON: u32 = 0xC;
pub const BS_DEFSPLITBUTTON: u32 = 0xD;
pub const BS_TYPEMASK: u32 = 0xF;

pub const SS_LEFT: u32 = 0x0;
pub const SS_LEFTNOWORDWRAP: u32 = 0xC;
pub const SS_NOPREFIX: u32 = 0x80;
pub const SS_CENTERIMAGE: u32 = 0x200;
pub const SS_ENDELLIPSIS: u32 = 0x4000;

pub const ES_AUTOHSCROLL: u32 = 0x80;
pub const ES_NUMBER: u32 = 0x2000;

pub const CBS_DROPDOWNLIST: u32 = 0x3;
pub const CBS_HASSTRINGS: u32 = 0x200;

pub const LBS_NOTIFY: u32 = 0x1;
pub const LBS_OWNERDRAWFIXED: u32 = 0x10;
pub const LBS_HASSTRINGS: u32 = 0x40;
pub const LBS_NOINTEGRALHEIGHT: u32 = 0x100;

pub const LVS_REPORT: u32 = 0x1;
pub const LVS_SINGLESEL: u32 = 0x4;
pub const LVS_SHOWSELALWAYS: u32 = 0x8;
pub const LVS_SHAREIMAGELISTS: u32 = 0x40;
pub const LVS_NOCOLUMNHEADER: u32 = 0x4000;

pub const UDS_WRAP: u32 = 0x1;
pub const UDS_SETBUDDYINT: u32 = 0x2;
pub const UDS_ALIGNRIGHT: u32 = 0x4;
pub const UDS_AUTOBUDDY: u32 = 0x10;
pub const UDS_ARROWKEYS: u32 = 0x20;
pub const UDS_NOTHOUSANDS: u32 = 0x80;
pub const UDS_HOTTRACK: u32 = 0x100;

pub const UPDOWN_STYLE: u32 =
    UDS_ALIGNRIGHT | UDS_ARROWKEYS | UDS_AUTOBUDDY | UDS_HOTTRACK | UDS_SETBUDDYINT | UDS_NOTHOUSANDS;

pub const CLASS_BUTTON: &str = "Button";
pub const CLASS_STATIC: &str = "Static";
pub const CLASS_EDIT: &str = "Edit";
pub const CLASS_COMBOBOX: &str = "ComboBox";
pub const CLASS_LISTBOX: &str = "ListBox";
pub const CLASS_LISTVIEW: &str = "SysListView32";
pub const CLASS_UPDOWN: &str = "msctls_updown32";
pub const CLASS_HOTKEY: &str = "msctls_hotkey32";

pub struct DialogTemplate {
    buf: Vec<u16>,
    count_pos: usize,
    count: u16,
}

impl DialogTemplate {
    /// Coordinates are dialog units. `DS_SETFONT` is added automatically.
    pub fn new(style: u32, ex_style: u32, x: i16, y: i16, cx: i16, cy: i16, title: &str) -> Self {
        let mut t = DialogTemplate { buf: Vec::with_capacity(1024), count_pos: 0, count: 0 };
        t.buf.push(1); // dlgVer
        t.buf.push(0xFFFF); // signature -> DLGTEMPLATEEX
        t.u32(0); // helpID
        t.u32(ex_style);
        t.u32(style | DS_SETFONT);
        t.count_pos = t.buf.len();
        t.buf.push(0); // cDlgItems, patched in build()
        t.i16(x);
        t.i16(y);
        t.i16(cx);
        t.i16(cy);
        t.buf.push(0); // no menu
        t.buf.push(0); // default dialog class
        t.str(title);
        t.buf.push(9); // point size
        t.buf.push(400); // weight
        t.buf.push(0x0100); // italic = 0, charset = DEFAULT_CHARSET (1)
        t.str("Segoe UI");
        t
    }

    fn u32(&mut self, v: u32) {
        self.buf.push((v & 0xFFFF) as u16);
        self.buf.push((v >> 16) as u16);
    }

    fn i16(&mut self, v: i16) {
        self.buf.push(v as u16);
    }

    fn str(&mut self, s: &str) {
        self.buf.extend(s.encode_utf16());
        self.buf.push(0);
    }

    fn align(&mut self) {
        if !self.buf.len().is_multiple_of(2) {
            self.buf.push(0);
        }
    }

    /// Adds a control; `WS_CHILD | WS_VISIBLE` are always set.
    #[allow(clippy::too_many_arguments)]
    pub fn item(
        &mut self,
        class: &str,
        text: &str,
        id: i32,
        style: u32,
        ex_style: u32,
        x: i16,
        y: i16,
        cx: i16,
        cy: i16,
    ) -> &mut Self {
        self.align();
        self.u32(0); // helpID
        self.u32(ex_style);
        self.u32(style | WS_CHILD | WS_VISIBLE);
        self.i16(x);
        self.i16(y);
        self.i16(cx);
        self.i16(cy);
        self.u32(id as u32);
        self.str(class);
        self.str(text);
        self.buf.push(0); // no creation data
        self.count += 1;
        self
    }

    // Convenience wrappers ---------------------------------------------------

    pub fn label(&mut self, id: i32, text: &str, x: i16, y: i16, cx: i16, cy: i16) -> &mut Self {
        self.item(CLASS_STATIC, text, id, SS_LEFTNOWORDWRAP | SS_CENTERIMAGE | SS_NOPREFIX, 0, x, y, cx, cy)
    }

    pub fn checkbox(&mut self, id: i32, text: &str, x: i16, y: i16, cx: i16, cy: i16) -> &mut Self {
        self.item(CLASS_BUTTON, text, id, BS_AUTOCHECKBOX | WS_TABSTOP, 0, x, y, cx, cy)
    }

    pub fn button(&mut self, id: i32, text: &str, x: i16, y: i16, cx: i16, cy: i16) -> &mut Self {
        self.item(CLASS_BUTTON, text, id, BS_PUSHBUTTON | WS_TABSTOP, 0, x, y, cx, cy)
    }

    /// Number edit with an attached up-down control.
    pub fn number(&mut self, edit_id: i32, updown_id: i32, x: i16, y: i16, cx: i16, cy: i16) -> &mut Self {
        self.item(CLASS_EDIT, "", edit_id, ES_AUTOHSCROLL | ES_NUMBER | WS_TABSTOP | WS_BORDER, 0, x, y, cx, cy);
        self.item(CLASS_UPDOWN, "", updown_id, UPDOWN_STYLE, 0, 0, 0, 0, 0)
    }

    pub fn combo(&mut self, id: i32, x: i16, y: i16, cx: i16, cy: i16) -> &mut Self {
        self.item(CLASS_COMBOBOX, "", id, CBS_DROPDOWNLIST | CBS_HASSTRINGS | WS_VSCROLL | WS_TABSTOP, 0, x, y, cx, cy)
    }

    pub fn listview(&mut self, id: i32, extra_style: u32, x: i16, y: i16, cx: i16, cy: i16) -> &mut Self {
        self.item(
            CLASS_LISTVIEW,
            "",
            id,
            LVS_REPORT | LVS_NOCOLUMNHEADER | LVS_SHAREIMAGELISTS | LVS_SINGLESEL | WS_TABSTOP | extra_style,
            0,
            x,
            y,
            cx,
            cy,
        )
    }

    /// Finished template, DWORD-aligned as the dialog manager requires.
    pub fn build(mut self) -> Vec<u32> {
        self.buf[self.count_pos] = self.count;
        self.align();
        let mut out = vec![0u32; self.buf.len() / 2];
        unsafe {
            std::ptr::copy_nonoverlapping(
                self.buf.as_ptr() as *const u8,
                out.as_mut_ptr() as *mut u8,
                self.buf.len() * 2,
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_layout() {
        let mut t = DialogTemplate::new(WS_POPUP, 0, 0, 0, 100, 50, "T");
        t.button(1, "OK", 1, 2, 3, 4);
        let raw = t.build();
        let words: Vec<u16> = raw.iter().flat_map(|d| [(*d & 0xFFFF) as u16, (*d >> 16) as u16]).collect();
        assert_eq!(words[0], 1);
        assert_eq!(words[1], 0xFFFF);
        assert_eq!(words[8], 1); // one item
        assert_eq!(words[9..13], [0, 0, 100, 50]);
    }
}
