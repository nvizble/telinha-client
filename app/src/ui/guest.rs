use super::*;
use std::ops::Range;

pub(super) struct GuestPrompt {
    pub(super) code: RoomCode,
    input: crate::guest::NameInput,
    error: Option<String>,
    line: Option<ShapedLine>,
    bounds: Option<Bounds<Pixels>>,
    offset: Pixels,
    selecting: bool,
}

impl Telinha {
    pub(super) fn ask_guest_name(&mut self, code: RoomCode, cx: &mut Context<Self>) {
        if let Some(prompt) = &mut self.guest_prompt {
            // A newer invitation changes the destination, not the name being typed.
            prompt.code = code;
        } else {
            self.guest_prompt = Some(GuestPrompt {
                code,
                input: crate::guest::NameInput::new(self.guest_name.clone().unwrap_or_default()),
                error: None,
                line: None,
                bounds: None,
                offset: px(0.),
                selecting: false,
            });
        }
        self.error = None;
        cx.notify();
    }

    fn cancel_guest_name(&mut self, cx: &mut Context<Self>) {
        if let Some(prompt) = self.guest_prompt.take()
            && self.connection == Connection::Home
        {
            self.input = prompt.code.as_str().to_owned();
            self.cursor = self.input.len().min(11);
            self.select_all = false;
        }
        cx.notify();
    }

    fn confirm_guest_name(&mut self, cx: &mut Context<Self>) {
        if self.auth_busy {
            return;
        }
        let Some(prompt) = &mut self.guest_prompt else {
            return;
        };
        if self.account.is_some() {
            let code = self.guest_prompt.take().unwrap().code;
            self.start_room(code, None, cx);
            return;
        }
        let name = match telinha_shared::normalize_guest_name(&prompt.input.text) {
            Ok(name) => name,
            Err(error) => {
                prompt.error = Some(error.into());
                cx.notify();
                return;
            }
        };
        let code = self.guest_prompt.take().unwrap().code;
        self.guest_name = Some(name.clone());
        let saved = crate::guest::save_name(&name).is_ok();
        self.start_room(code, Some(name), cx);
        if !saved {
            self.toast("Não foi possível lembrar seu nome neste computador.".into());
        }
    }

    pub(super) fn guest_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = &event.keystroke;
        if key.key == "escape" {
            self.cancel_guest_name(cx);
            window.prevent_default();
            cx.stop_propagation();
            return;
        }
        let Some(prompt) = &mut self.guest_prompt else {
            return;
        };
        // Let the platform finish an IME composition before Enter submits.
        if prompt.input.marked.is_some() {
            return;
        }
        let mut handled = true;
        if key.modifiers.control || key.modifiers.platform {
            match key.key.as_str() {
                "a" => {
                    prompt.input.move_to(0, false);
                    prompt.input.move_to(prompt.input.text.len(), true);
                }
                "v" => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        if !prompt.input.replace(None, &text, None) {
                            prompt.error = Some(
                                "Use um nome em uma linha, sem caracteres de controle.".into(),
                            );
                        } else {
                            prompt.error = None;
                        }
                    }
                }
                "c" | "x" => {
                    if !prompt.input.selection.is_empty() {
                        cx.write_to_clipboard(ClipboardItem::new_string(
                            prompt.input.text[prompt.input.selection.clone()].to_owned(),
                        ));
                        if key.key == "x" {
                            prompt.input.replace(None, "", None);
                        }
                    }
                }
                _ => handled = false,
            }
        } else {
            match key.key.as_str() {
                "enter" => self.confirm_guest_name(cx),
                "left" => {
                    let at = if !key.modifiers.shift && !prompt.input.selection.is_empty() {
                        prompt.input.selection.start
                    } else {
                        prompt.input.previous(prompt.input.cursor())
                    };
                    prompt.input.move_to(at, key.modifiers.shift);
                }
                "right" => {
                    let at = if !key.modifiers.shift && !prompt.input.selection.is_empty() {
                        prompt.input.selection.end
                    } else {
                        prompt.input.next(prompt.input.cursor())
                    };
                    prompt.input.move_to(at, key.modifiers.shift);
                }
                "home" => prompt.input.move_to(0, key.modifiers.shift),
                "end" => prompt
                    .input
                    .move_to(prompt.input.text.len(), key.modifiers.shift),
                "backspace" | "delete" => {
                    if prompt.input.selection.is_empty() {
                        let at = if key.key == "backspace" {
                            prompt.input.previous(prompt.input.cursor())
                        } else {
                            prompt.input.next(prompt.input.cursor())
                        };
                        prompt.input.move_to(at, true);
                    }
                    prompt.input.replace(None, "", None);
                    prompt.error = None;
                }
                _ => handled = false,
            }
        }
        if handled {
            window.prevent_default();
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn guest_index(&self, position: Point<Pixels>) -> usize {
        let Some(prompt) = &self.guest_prompt else {
            return 0;
        };
        let (Some(line), Some(bounds)) = (&prompt.line, prompt.bounds) else {
            return 0;
        };
        if prompt.input.text.is_empty() {
            return 0;
        }
        line.closest_index_for_x(position.x - bounds.left() + prompt.offset)
            .min(prompt.input.text.len())
    }

    pub(super) fn guest_form(&self, cx: &mut Context<Self>) -> AnyElement {
        let prompt = self.guest_prompt.as_ref().expect("guest form is open");
        let count = prompt.input.text.trim().chars().count();
        let error = prompt.error.clone();
        let view = cx.entity();
        let field = div()
            .id("guest-name-input")
            .w_full()
            .h(px(54.))
            .px_4()
            .py(px(14.))
            .rounded(px(9.))
            .bg(rgb(SUNK))
            .border_1()
            .border_color(rgb(if error.is_some() { WARN } else { FG }))
            .text_size(px(18.))
            .line_height(px(24.))
            .cursor(CursorStyle::IBeam)
            .overflow_hidden()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    let at = this.guest_index(event.position);
                    if let Some(prompt) = &mut this.guest_prompt {
                        prompt.input.move_to(at, event.modifiers.shift);
                        prompt.selecting = true;
                    }
                    window.focus(&this.focus);
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if event.pressed_button == Some(MouseButton::Left)
                    && this
                        .guest_prompt
                        .as_ref()
                        .is_some_and(|prompt| prompt.selecting)
                {
                    let at = this.guest_index(event.position);
                    if let Some(prompt) = &mut this.guest_prompt {
                        prompt.input.move_to(at, true);
                    }
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    if let Some(prompt) = &mut this.guest_prompt {
                        prompt.selecting = false;
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    if let Some(prompt) = &mut this.guest_prompt {
                        prompt.selecting = false;
                    }
                }),
            )
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, (), window, cx| {
                        let (focus, text, selection, cursor, marked) = {
                            let app = view.read(cx);
                            let Some(prompt) = &app.guest_prompt else {
                                return;
                            };
                            (
                                app.focus.clone(),
                                prompt.input.text.clone(),
                                prompt.input.selection.clone(),
                                prompt.input.cursor(),
                                prompt.input.marked.clone(),
                            )
                        };
                        let style = window.text_style();
                        let placeholder = text.is_empty();
                        let display: SharedString = if placeholder {
                            "Seu nome ou apelido".into()
                        } else {
                            text.clone().into()
                        };
                        let run = TextRun {
                            len: display.len(),
                            font: style.font(),
                            color: rgb(if placeholder { MUTED } else { FG }).into(),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        let runs = if let Some(marked) = marked {
                            vec![
                                TextRun {
                                    len: marked.start,
                                    ..run.clone()
                                },
                                TextRun {
                                    len: marked.len(),
                                    underline: Some(UnderlineStyle {
                                        color: Some(run.color),
                                        thickness: px(1.),
                                        wavy: false,
                                    }),
                                    ..run.clone()
                                },
                                TextRun {
                                    len: display.len() - marked.end,
                                    ..run
                                },
                            ]
                            .into_iter()
                            .filter(|run| run.len > 0)
                            .collect()
                        } else {
                            vec![run]
                        };
                        let line = window.text_system().shape_line(
                            display,
                            style.font_size.to_pixels(window.rem_size()),
                            &runs,
                            None,
                        );
                        let offset =
                            (line.x_for_index(cursor) - bounds.size.width + px(3.)).max(px(0.));
                        let origin = point(bounds.left() - offset, bounds.top());
                        window.handle_input(
                            &focus,
                            ElementInputHandler::new(bounds, view.clone()),
                            cx,
                        );
                        window.with_content_mask(Some(ContentMask { bounds }), |window| {
                            if !selection.is_empty() {
                                window.paint_quad(fill(
                                    Bounds::from_corners(
                                        point(
                                            origin.x + line.x_for_index(selection.start),
                                            bounds.top(),
                                        ),
                                        point(
                                            origin.x + line.x_for_index(selection.end),
                                            bounds.bottom(),
                                        ),
                                    ),
                                    rgba(0x5865F266),
                                ));
                            }
                            let _ = line.paint(origin, px(24.), window, cx);
                            if focus.is_focused(window) && selection.is_empty() {
                                window.paint_quad(fill(
                                    Bounds::new(
                                        point(origin.x + line.x_for_index(cursor), bounds.top()),
                                        size(px(1.5), bounds.size.height),
                                    ),
                                    rgb(FG),
                                ));
                            }
                        });
                        view.update(cx, |app, _| {
                            if let Some(prompt) = &mut app.guest_prompt {
                                prompt.line = Some(line);
                                prompt.bounds = Some(bounds);
                                prompt.offset = offset;
                            }
                        });
                    },
                )
                .w_full()
                .h(px(24.)),
            );
        div().flex_1().min_h_0().flex().items_center().justify_center().p_8()
            .child(div().w(px(520.)).max_w_full().flex().flex_col().gap_5()
                .child(div().size(px(52.)).rounded(px(14.)).bg(rgb(SUNK)).flex().items_center().justify_center().child(icon("icons/people.svg", 25., FG)))
                .child(heading("Como podemos chamar você?"))
                .child(div().text_color(rgb(MUTED)).line_height(px(22.)).child("Escolha o nome que seus amigos vão ver na sessão."))
                .child(div().mt_2().flex().items_center().justify_between().child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("Seu nome"))
                    .child(div().text_xs().text_color(rgb(if count > telinha_shared::MAX_GUEST_NAME_CHARS { WARN } else { MUTED })).child(format!("{count}/{}", telinha_shared::MAX_GUEST_NAME_CHARS))))
                .child(field)
                .when_some(error, |form, error| form.child(div().text_sm().text_color(rgb(WARN)).child(error)))
                .child(div().text_xs().text_color(rgb(MUTED)).line_height(px(19.)).child("Vamos lembrar neste computador. Você pode editar antes de entrar novamente."))
                .child(div().pt_3().flex().items_center().gap_3()
                    .child(button("guest-name-confirm", "Entrar na sessão", true).on_click(cx.listener(|this, _, _, cx| this.confirm_guest_name(cx))))
                    .child(quiet_button("guest-name-cancel", "Cancelar").on_click(cx.listener(|this, _, _, cx| this.cancel_guest_name(cx))))))
            .into_any_element()
    }
}

impl EntityInputHandler for Telinha {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let input = &self.guest_prompt.as_ref()?.input;
        let range = input.from_utf16(range);
        *actual = Some(input.to_utf16(range.clone()));
        input.text.get(range).map(str::to_owned)
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let input = &self.guest_prompt.as_ref()?.input;
        Some(UTF16Selection {
            range: input.to_utf16(input.selection.clone()),
            reversed: input.reversed,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        let input = &self.guest_prompt.as_ref()?.input;
        input.marked.clone().map(|range| input.to_utf16(range))
    }
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(prompt) = &mut self.guest_prompt {
            prompt.input.marked = None;
            cx.notify();
        }
    }
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(prompt) = &mut self.guest_prompt else {
            return;
        };
        let range = range.map(|range| prompt.input.from_utf16(range));
        if prompt.input.replace(range, text, None) {
            prompt.error = None;
        } else {
            prompt.error = Some("Use um nome em uma linha, sem caracteres de controle.".into());
        }
        cx.notify();
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(prompt) = &mut self.guest_prompt else {
            return;
        };
        let range = range.map(|range| prompt.input.from_utf16(range));
        let end = text.encode_utf16().count();
        if prompt
            .input
            .replace(range, text, Some(selected.unwrap_or(end..end)))
        {
            prompt.error = None;
        }
        cx.notify();
    }
    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let prompt = self.guest_prompt.as_ref()?;
        let line = prompt.line.as_ref()?;
        let range = prompt.input.from_utf16(range);
        Some(Bounds::from_corners(
            point(
                bounds.left() - prompt.offset + line.x_for_index(range.start),
                bounds.top(),
            ),
            point(
                bounds.left() - prompt.offset + line.x_for_index(range.end),
                bounds.bottom(),
            ),
        ))
    }
    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let prompt = self.guest_prompt.as_ref()?;
        let at = self.guest_index(point);
        Some(prompt.input.to_utf16(at..at).start)
    }
}
