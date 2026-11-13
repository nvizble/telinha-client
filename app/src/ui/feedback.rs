use super::*;
use crate::feedback::{Draft, Status, Submission, editor::CommentInput};
use std::ops::Range;

pub(super) struct Closed(pub u64);
pub(super) struct FeedbackDialog {
    draft: Draft,
    sender: tokio::sync::mpsc::UnboundedSender<Submission>,
    input: CommentInput,
    focus: FocusHandle,
    return_focus: FocusHandle,
    focus_requested: bool,
    lines: Vec<PositionedLine>,
    bounds: Option<Bounds<Pixels>>,
    scroll: Pixels,
    content_height: Pixels,
    ensure_caret: bool,
    selecting: bool,
    input_error: Option<&'static str>,
}
struct PositionedLine {
    line: WrappedLine,
    start: usize,
    top: Pixels,
}
impl EventEmitter<Closed> for FeedbackDialog {}
impl FeedbackDialog {
    fn new(
        draft: Draft,
        sender: tokio::sync::mpsc::UnboundedSender<Submission>,
        return_focus: FocusHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            draft,
            sender,
            input: CommentInput::default(),
            focus: cx.focus_handle(),
            return_focus,
            focus_requested: true,
            lines: vec![],
            bounds: None,
            scroll: px(0.),
            content_height: px(0.),
            ensure_caret: true,
            selecting: false,
            input_error: None,
        }
    }
    pub(super) fn id(&self) -> u64 {
        self.draft.id
    }
    pub(super) fn completed(&mut self, result: Result<(), String>, cx: &mut Context<Self>) {
        self.draft.completed(result);
        cx.notify();
    }
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.return_focus);
        cx.emit(Closed(self.draft.id));
    }
    fn submit(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.draft.submit(&self.input.text) else {
            return;
        };
        self.input.marked = None;
        if self
            .sender
            .send(Submission {
                id: self.draft.id,
                request,
            })
            .is_err()
        {
            self.draft
                .completed(Err("Não foi possível enviar agora. Tente novamente.".into()));
        }
        cx.notify();
    }
    fn replace(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        marked: Option<Range<usize>>,
        cx: &mut Context<Self>,
    ) {
        if !self.draft.editable() {
            return;
        }
        self.input_error = if self.input.replace(range, text, marked) {
            None
        } else {
            Some("Use até 2.000 caracteres, sem caracteres de controle.")
        };
        self.ensure_caret = true;
        cx.notify();
    }
    fn point_for(&self, index: usize) -> Point<Pixels> {
        line_position(&self.lines, index)
    }
    fn index_at(&self, position: Point<Pixels>) -> usize {
        let Some(bounds) = self.bounds else {
            return 0;
        };
        self.index_in_content(point(
            position.x - bounds.left(),
            position.y - bounds.top() + self.scroll,
        ))
    }
    fn index_in_content(&self, position: Point<Pixels>) -> usize {
        for entry in &self.lines {
            let height = line_height(&entry.line);
            if position.y < entry.top + height {
                let at = entry
                    .line
                    .closest_index_for_position(
                        point(position.x, (position.y - entry.top).max(px(0.))),
                        px(20.),
                    )
                    .unwrap_or_else(|at| at);
                return (entry.start + at).min(self.input.text.len());
            }
        }
        self.input.text.len()
    }
    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = &event.keystroke;
        let command = key.modifiers.control || key.modifiers.platform;
        if (command && matches!(key.key.as_str(), "q" | "w"))
            || (key.modifiers.alt && key.key == "f4")
        {
            return;
        }
        cx.stop_propagation();
        // Let the platform cancel or commit an active composition first.
        if self.input.marked.is_some() {
            return;
        }
        if key.key == "escape" {
            self.close(window, cx);
            window.prevent_default();
            return;
        }
        let mut handled = true;
        if command {
            match key.key.as_str() {
                "a" => {
                    self.input.move_to(0, false);
                    let end = self.input.text.len();
                    self.input.move_to(end, true);
                }
                "c" | "x" => {
                    if !self.input.selection.is_empty() {
                        cx.write_to_clipboard(ClipboardItem::new_string(
                            self.input.text[self.input.selection.clone()].to_owned(),
                        ));
                        if key.key == "x" {
                            self.replace(None, "", None, cx);
                        }
                    }
                }
                "v" => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        self.replace(None, &text, None, cx);
                    }
                }
                "home" => self.input.move_to(0, key.modifiers.shift),
                "end" => {
                    let end = self.input.text.len();
                    self.input.move_to(end, key.modifiers.shift);
                }
                "enter" => self.submit(cx),
                _ => handled = false,
            }
        } else {
            match key.key.as_str() {
                "enter" => self.replace(None, "\n", None, cx),
                "left" | "right" => {
                    let at = if !key.modifiers.shift && !self.input.selection.is_empty() {
                        if key.key == "left" {
                            self.input.selection.start
                        } else {
                            self.input.selection.end
                        }
                    } else if key.key == "left" {
                        self.input.previous(self.input.cursor())
                    } else {
                        self.input.next(self.input.cursor())
                    };
                    self.input.move_to(at, key.modifiers.shift);
                }
                "up" | "down" => {
                    let point = self.point_for(self.input.cursor());
                    let y = if key.key == "up" {
                        point.y - px(20.)
                    } else {
                        point.y + px(20.)
                    };
                    let at = self.index_in_content(gpui::point(point.x, y.max(px(0.))));
                    self.input.move_to(at, key.modifiers.shift);
                }
                "home" => {
                    let at = self.input.line_start();
                    self.input.move_to(at, key.modifiers.shift);
                }
                "end" => {
                    let at = self.input.line_end();
                    self.input.move_to(at, key.modifiers.shift);
                }
                "backspace" | "delete" if self.draft.editable() => {
                    if self.input.selection.is_empty() {
                        let at = if key.key == "backspace" {
                            self.input.previous(self.input.cursor())
                        } else {
                            self.input.next(self.input.cursor())
                        };
                        self.input.move_to(at, true);
                    }
                    self.replace(None, "", None, cx);
                }
                _ => handled = false,
            }
        }
        if handled {
            self.ensure_caret = true;
            window.prevent_default();
            cx.notify();
        }
    }
    fn rating(&self, audio: bool, cx: &mut Context<Self>) -> AnyElement {
        let value = if audio {
            self.draft.audio_rating
        } else {
            self.draft.video_rating
        };
        let prefix = if audio {
            "feedback-audio"
        } else {
            "feedback-video"
        };
        let mut stars = div().flex().items_center().gap_1();
        for rating in 1..=5u8 {
            stars = stars.child(
                div()
                    .id((prefix, usize::from(rating)))
                    .size(px(34.))
                    .rounded(px(6.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(PANEL)))
                    .when(!self.draft.editable(), |star| {
                        star.opacity(0.5).cursor_default()
                    })
                    .child(icon(
                        if value.is_some_and(|value| rating <= value) {
                            "icons/star-filled.svg"
                        } else {
                            "icons/star.svg"
                        },
                        23.,
                        if value.is_some_and(|value| rating <= value) {
                            WARN
                        } else {
                            MUTED
                        },
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.draft.editable() {
                            if audio {
                                this.draft.audio_rating = Some(rating);
                            } else {
                                this.draft.video_rating = Some(rating);
                            }
                            cx.notify();
                        }
                    })),
            );
        }
        div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(if audio {
                                "Qualidade do áudio"
                            } else {
                                "Qualidade do vídeo"
                            }),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id((prefix, 0usize))
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .cursor_pointer()
                            .child(if value.is_none() {
                                "Não avaliei"
                            } else {
                                "Limpar nota"
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.draft.editable() {
                                    if audio {
                                        this.draft.audio_rating = None;
                                    } else {
                                        this.draft.video_rating = None;
                                    }
                                    cx.notify();
                                }
                            })),
                    ),
            )
            .child(
                div().flex().items_center().gap_3().child(stars).child(
                    div().text_xs().text_color(rgb(MUTED)).child(
                        value.map_or_else(|| "Opcional".into(), |value| format!("{value}/5")),
                    ),
                ),
            )
            .into_any_element()
    }
    fn comment_field(&self, cx: &mut Context<Self>) -> AnyElement {
        let view = cx.entity();
        div()
            .id("feedback-comment")
            .w_full()
            .h(px(132.))
            .flex_shrink_0()
            .px_3()
            .py_3()
            .rounded(px(8.))
            .border_1()
            .border_color(rgb(if self.input_error.is_some() {
                WARN
            } else {
                LINE
            }))
            .bg(rgb(SUNK))
            .overflow_hidden()
            .cursor(CursorStyle::IBeam)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    let at = this.index_at(event.position);
                    this.input.move_to(at, event.modifiers.shift);
                    this.selecting = true;
                    this.ensure_caret = true;
                    window.focus(&this.focus);
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if this.selecting && event.pressed_button == Some(MouseButton::Left) {
                    let at = this.index_at(event.position);
                    this.input.move_to(at, true);
                    this.ensure_caret = true;
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.selecting = false),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.selecting = false),
            )
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                let height = this.bounds.map_or(px(108.), |bounds| bounds.size.height);
                this.scroll = (this.scroll - event.delta.pixel_delta(px(20.)).y)
                    .clamp(px(0.), (this.content_height - height).max(px(0.)));
                this.ensure_caret = false;
                cx.stop_propagation();
                cx.notify();
            }))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, (), window, cx| {
                        let (text, selection, cursor, marked, focus, old_scroll, ensure_caret) = {
                            let this = view.read(cx);
                            (
                                this.input.text.clone(),
                                this.input.selection.clone(),
                                this.input.cursor(),
                                this.input.marked.clone(),
                                this.focus.clone(),
                                this.scroll,
                                this.ensure_caret,
                            )
                        };
                        let placeholder = text.is_empty();
                        let display: SharedString = if placeholder {
                            "O que funcionou bem? O que podemos melhorar?".into()
                        } else {
                            text.clone().into()
                        };
                        let style = window.text_style();
                        let base = TextRun {
                            len: display.len(),
                            font: style.font(),
                            color: rgb(if placeholder { MUTED } else { FG }).into(),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        let mut cuts = vec![0, display.len()];
                        if !placeholder {
                            cuts.extend([selection.start, selection.end]);
                            if let Some(marked) = &marked {
                                cuts.extend([marked.start, marked.end]);
                            }
                        }
                        cuts.sort_unstable();
                        cuts.dedup();
                        let runs: Vec<_> = cuts
                            .windows(2)
                            .filter(|range| range[0] < range[1])
                            .map(|range| TextRun {
                                len: range[1] - range[0],
                                background_color: (!placeholder
                                    && range[0] >= selection.start
                                    && range[1] <= selection.end)
                                    .then_some(rgba(0x5865F266).into()),
                                underline: marked
                                    .as_ref()
                                    .filter(|marked| {
                                        range[0] >= marked.start && range[1] <= marked.end
                                    })
                                    .map(|_| UnderlineStyle {
                                        color: Some(rgb(FG).into()),
                                        thickness: px(1.),
                                        wavy: false,
                                    }),
                                ..base.clone()
                            })
                            .collect();
                        let Ok(shaped) = window.text_system().shape_text(
                            display,
                            px(14.),
                            &runs,
                            Some(bounds.size.width),
                            None,
                        ) else {
                            return;
                        };
                        let mut start = 0;
                        let mut top = px(0.);
                        let mut lines = Vec::new();
                        for line in shaped {
                            let height = line_height(&line);
                            let len = line.len();
                            lines.push(PositionedLine { line, start, top });
                            start += len + 1;
                            top += height;
                        }
                        let caret = line_position(&lines, if placeholder { 0 } else { cursor });
                        let mut scroll =
                            old_scroll.clamp(px(0.), (top - bounds.size.height).max(px(0.)));
                        if ensure_caret {
                            if caret.y < scroll {
                                scroll = caret.y;
                            } else if caret.y + px(20.) > scroll + bounds.size.height {
                                scroll = caret.y + px(20.) - bounds.size.height;
                            }
                        }
                        window.handle_input(
                            &focus,
                            ElementInputHandler::new(bounds, view.clone()),
                            cx,
                        );
                        window.with_content_mask(Some(ContentMask { bounds }), |window| {
                            for entry in &lines {
                                let origin =
                                    point(bounds.left(), bounds.top() + entry.top - scroll);
                                let _ = entry.line.paint_background(
                                    origin,
                                    px(20.),
                                    TextAlign::Left,
                                    Some(bounds),
                                    window,
                                    cx,
                                );
                                let _ = entry.line.paint(
                                    origin,
                                    px(20.),
                                    TextAlign::Left,
                                    Some(bounds),
                                    window,
                                    cx,
                                );
                            }
                            if focus.is_focused(window) && selection.is_empty() {
                                window.paint_quad(fill(
                                    Bounds::new(
                                        point(
                                            bounds.left() + caret.x,
                                            bounds.top() + caret.y - scroll,
                                        ),
                                        size(px(1.5), px(20.)),
                                    ),
                                    rgb(FG),
                                ));
                            }
                        });
                        view.update(cx, |this, _| {
                            this.lines = lines;
                            this.bounds = Some(bounds);
                            this.scroll = scroll;
                            this.content_height = top;
                            this.ensure_caret = false;
                        });
                    },
                )
                .w_full()
                .h(px(108.)),
            )
            .into_any_element()
    }
}
fn line_height(line: &WrappedLine) -> Pixels {
    px(20.) * (line.wrap_boundaries.len() + 1) as f32
}
fn line_position(lines: &[PositionedLine], index: usize) -> Point<Pixels> {
    for entry in lines {
        if index >= entry.start && index <= entry.start + entry.line.len() {
            let position = entry
                .line
                .position_for_index(index - entry.start, px(20.))
                .unwrap_or_default();
            return point(position.x, entry.top + position.y);
        }
    }
    lines.last().map_or(point(px(0.), px(0.)), |entry| {
        let position = entry
            .line
            .position_for_index(entry.line.len(), px(20.))
            .unwrap_or_default();
        point(position.x, entry.top + position.y)
    })
}
impl Render for FeedbackDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_requested {
            window.focus(&self.focus);
            self.focus_requested = false;
        }
        let sent = self.draft.status == Status::Sent;
        let sending = self.draft.status == Status::Sending;
        let mut form = div()
            .id("feedback-dialog")
            .w(px(560.))
            .max_w_full()
            .max_h((window.viewport_size().height - px(48.)).max(px(160.)))
            .overflow_y_scroll()
            .p_6()
            .rounded(px(16.))
            .border_1()
            .border_color(rgb(LINE))
            .bg(rgb(BG))
            .shadow_lg()
            .flex()
            .flex_col()
            .gap_4()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(24.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(if sent {
                                "Obrigado pelo feedback!"
                            } else if self.draft.during_sharing() {
                                "Como está a transmissão?"
                            } else {
                                "Como foi sua sessão?"
                            }),
                    )
                    .child(
                        quiet_button("feedback-close", "Fechar")
                            .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .truncate()
                    .child(self.draft.label.clone()),
            );
        if sent {
            return form
                .child(
                    div()
                        .flex_shrink_0()
                        .text_sm()
                        .line_height(px(22.))
                        .child("Sua avaliação foi enviada à equipe da telinha."),
                )
                .child(
                    button("feedback-done", "Concluir", true)
                        .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))),
                );
        }
        if self.draft.during_sharing() {
            form = form.child(
                div()
                    .flex_shrink_0()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child("Você pode responder sem interromper a transmissão."),
            );
        }
        form=form.child(div().flex_shrink_0().text_sm().line_height(px(21.)).text_color(rgb(MUTED)).child("A avaliação é opcional e fica visível à equipe junto com a identificação desta sessão."))
            .child(self.rating(true,cx)).child(self.rating(false,cx))
            .child(div().flex_shrink_0().flex().items_center().child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("Comentário (opcional)")).child(div().flex_1()).child(div().text_xs().text_color(rgb(MUTED)).child(format!("{}/2.000",self.input.text.chars().count()))))
            .child(self.comment_field(cx));
        if let Some(error) = self.input_error {
            form = form.child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(rgb(WARN))
                    .child(error),
            );
        }
        if let Status::Failed(message) = &self.draft.status {
            form = form.child(
                div()
                    .flex_shrink_0()
                    .text_sm()
                    .line_height(px(20.))
                    .text_color(rgb(WARN))
                    .child(message.clone()),
            );
        }
        let can_send = self.draft.can_send(&self.input.text);
        form.child(
            div()
                .flex_shrink_0()
                .text_xs()
                .text_color(rgb(MUTED))
                .child(if sending {
                    "Enviando… Você pode fechar e continuar usando a telinha."
                } else if !self.draft.editable() {
                    "Tentar novamente envia as mesmas respostas."
                } else {
                    "Avalie ao menos uma categoria ou deixe um comentário."
                }),
        )
        .child(
            div()
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    button(
                        "feedback-send",
                        if sending {
                            "Enviando…"
                        } else if !self.draft.editable() {
                            "Tentar novamente"
                        } else {
                            "Enviar avaliação"
                        },
                        true,
                    )
                    .when(!can_send, |button| button.opacity(0.45))
                    .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                )
                .child(
                    quiet_button("feedback-skip", "Agora não")
                        .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))),
                ),
        )
    }
}
impl EntityInputHandler for FeedbackDialog {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.input.from_utf16(range);
        *actual = Some(self.input.to_utf16(range.clone()));
        self.input.text.get(range).map(str::to_owned)
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.input.to_utf16(self.input.selection.clone()),
            reversed: self.input.reversed,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.input
            .marked
            .clone()
            .map(|range| self.input.to_utf16(range))
    }
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.input.marked = None;
        cx.notify();
    }
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range.map(|range| self.input.from_utf16(range));
        self.replace(range, text, None, cx);
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range.map(|range| self.input.from_utf16(range));
        let end = text.encode_utf16().count();
        self.replace(range, text, Some(selected.unwrap_or(end..end)), cx);
    }
    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.input.from_utf16(range);
        let point = self.point_for(range.start);
        Some(Bounds::new(
            gpui::point(
                bounds.left() + point.x,
                bounds.top() + point.y - self.scroll,
            ),
            size(px(2.), px(20.)),
        ))
    }
    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let at = self.index_at(point);
        Some(self.input.to_utf16(at..at).start)
    }
}
impl Telinha {
    pub(super) fn sharing_feedback_card(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("sharing-feedback-card")
            .absolute()
            .right(px(20.))
            .bottom(px(88.))
            .w(px(312.))
            .p_4()
            .rounded(px(12.))
            .border_1()
            .border_color(rgb(LINE))
            .bg(rgb(SUNK))
            .shadow_lg()
            .flex()
            .flex_col()
            .gap_3()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(icon("icons/star.svg", 18., WARN))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Como está a transmissão?"),
                    ),
            )
            .child(
                div()
                    .text_sm()
                    .line_height(px(20.))
                    .text_color(rgb(MUTED))
                    .child("Conte pra gente como estão áudio e imagem."),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        button("sharing-feedback-rate", "Avaliar", true)
                            .h(px(32.))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.feedback_dialog.is_none()
                                    && this
                                        .feedback
                                        .request_sharing(this.generation, Instant::now())
                                {
                                    this.present_feedback(cx);
                                    this.feedback_open = true;
                                    cx.notify();
                                }
                            })),
                    )
                    .child(
                        quiet_button("sharing-feedback-later", "Agora não")
                            .h(px(32.))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.feedback.dismiss_sharing_prompt(this.generation);
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }
    pub(super) fn present_feedback(&mut self, cx: &mut Context<Self>) {
        if self.feedback_dialog.is_some() {
            return;
        }
        let Some(draft) = self.feedback.pop_pending() else {
            return;
        };
        let sender = self.backend.feedback.commands.clone();
        let dialog = cx.new(|cx| FeedbackDialog::new(draft, sender, self.focus.clone(), cx));
        self.feedback_subscription = Some(cx.subscribe(&dialog, |this, _, closed: &Closed, cx| {
            if this
                .feedback_dialog
                .as_ref()
                .is_some_and(|dialog| dialog.read(cx).id() == closed.0)
            {
                this.feedback_dialog = None;
                this.feedback_open = false;
                this.feedback_subscription = None;
                cx.notify();
            }
        }));
        self.feedback_open = self.connection == Connection::Home && self.guest_prompt.is_none();
        self.feedback_dialog = Some(dialog);
        cx.notify();
    }
    pub(super) fn feedback_banner(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex_shrink_0()
            .px_6()
            .py_2()
            .bg(rgb(SUNK))
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child("Quer contar como foi a sessão anterior?"),
            )
            .child(
                quiet_button("open-feedback", "Avaliar sessão").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.feedback_open = true;
                        if let Some(dialog) = &this.feedback_dialog {
                            dialog.update(cx, |dialog, _| dialog.focus_requested = true);
                        }
                        cx.notify();
                    },
                )),
            )
            .child(
                quiet_button("skip-feedback", "Agora não").on_click(cx.listener(
                    |this, _, window, cx| {
                        this.feedback_dialog = None;
                        this.feedback_subscription = None;
                        this.feedback_open = false;
                        window.focus(&this.focus);
                        cx.notify();
                    },
                )),
            )
            .into_any_element()
    }
}
