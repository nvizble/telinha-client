use super::*;
use livekit::webrtc::desktop_capturer::DesktopCaptureSourceType as Kind;

impl Telinha {
    pub(super) fn picker(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let compact =
            window.viewport_size().width < px(1040.) || window.viewport_size().height < px(660.);
        let columns = if compact { 2 } else { 3 };
        let page_size = columns * 2;
        let side_width = if compact { 264. } else { 288. };
        let padding = if compact { 20. } else { 24. };
        let kind = if self.source_tab_screen {
            Kind::Screen
        } else {
            Kind::Window
        };
        let choices: Vec<_> = self
            .sources
            .iter()
            .enumerate()
            .filter(|(_, source)| source.kind == kind)
            .collect();
        let pages = choices.len().div_ceil(page_size).max(1);
        let page = self.source_page.min(pages - 1);
        let disabled = self.starting || self.loading_sources;

        let title = div()
            .flex_shrink_0()
            .h(px(if compact { 74. } else { 92. }))
            .px(px(padding))
            .flex()
            .flex_col()
            .justify_center()
            .gap_1()
            .child(
                div()
                    .text_size(px(if compact { 21. } else { 24. }))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(if self.publishing.is_some() {
                        "Qual tela você quer transmitir agora?"
                    } else {
                        "O que você quer compartilhar?"
                    }),
            )
            .child(div().text_size(px(12.)).text_color(rgb(MUTED)).child(
                if cfg!(target_os = "macos") {
                    "Escolha a qualidade. O macOS cuida da seleção da janela ou tela."
                } else {
                    "Escolha uma janela ou tela. O áudio você decide ao lado."
                },
            ));

        let mut sources = div()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(10.));
        if !cfg!(target_os = "macos") {
            let mut tabs = div()
                .h(px(36.))
                .p(px(3.))
                .rounded(px(8.))
                .bg(rgb(SUNK))
                .flex()
                .gap(px(3.));
            for (screen, label, path) in [
                (false, "Janelas", "icons/window.svg"),
                (true, "Telas", "icons/monitor.svg"),
            ] {
                let selected = screen == self.source_tab_screen;
                let count = self
                    .sources
                    .iter()
                    .filter(|source| (source.kind == Kind::Screen) == screen)
                    .count();
                tabs = tabs.child(
                    div()
                        .id(("source-tab", usize::from(screen)))
                        .h_full()
                        .px_3()
                        .rounded(px(6.))
                        .flex()
                        .items_center()
                        .gap_2()
                        .cursor_pointer()
                        .text_size(px(12.))
                        .text_color(rgb(if selected { FG } else { MUTED }))
                        .when(selected, |tab| tab.bg(rgb(PANEL)))
                        .when(disabled, |tab| tab.opacity(0.5).cursor_default())
                        .child(icon(path, 15., if selected { FG } else { MUTED }))
                        .child(label)
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(rgb(MUTED))
                                .child(count.to_string()),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.starting || this.loading_sources {
                                return;
                            }
                            this.source_tab_screen = screen;
                            this.source_page = 0;
                            this.selected_source = this
                                .sources
                                .iter()
                                .position(|source| (source.kind == Kind::Screen) == screen);
                            this.update_audio_for_source();
                            cx.notify();
                        })),
                );
            }
            sources = sources.child(
                div()
                    .h(px(36.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(tabs)
                    .child(
                        button_with_icon("refresh", "Atualizar", "icons/refresh.svg", false)
                            .h(px(34.))
                            .px_3()
                            .text_size(px(11.))
                            .when(disabled, |button| button.opacity(0.45))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if !this.starting && !this.loading_sources {
                                    this.choose_source(cx);
                                }
                            })),
                    ),
            );
        }
        if self.loading_sources {
            sources = sources.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .rounded(px(10.))
                    .bg(rgb(SUNK))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(MUTED))
                    .child("Procurando janelas e telas…"),
            );
        } else if cfg!(target_os = "macos") {
            sources = sources.child(div().flex_1().min_h_0().rounded(px(12.)).border_1().border_color(rgb(LINE))
                .bg(rgb(SUNK)).p_6().flex().flex_col().items_center().justify_center().gap_4()
                .child(icon("icons/window.svg", if compact { 44. } else { 64. }, MUTED))
                .child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).child("Sua janela, sua escolha"))
                .child(div().max_w(px(340.)).text_sm().line_height(px(21.)).text_center().text_color(rgb(MUTED))
                    .child("Ao começar, escolha uma janela ou tela no seletor do macOS. Só o conteúdo escolhido será compartilhado.")));
        } else if choices.is_empty() {
            sources = sources.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .rounded(px(10.))
                    .bg(rgb(SUNK))
                    .p_6()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child("Nenhuma fonte disponível. Abra um aplicativo e atualize a lista."),
            );
        } else {
            let visible: Vec<_> = choices
                .iter()
                .skip(page * page_size)
                .take(page_size)
                .collect();
            let mut rows = div()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(10.));
            for row in 0..2 {
                let mut line = div().flex_1().min_h_0().min_w_0().flex().gap(px(10.));
                for col in 0..columns {
                    if let Some((index, source)) = visible.get(row * columns + col) {
                        let index = *index;
                        let selected = self.selected_source == Some(index);
                        let source_id = source.id;
                        let path = if kind == Kind::Screen {
                            "icons/monitor.svg"
                        } else {
                            "icons/window.svg"
                        };
                        let preview = div()
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .bg(rgb(0x17191C))
                            .flex()
                            .items_center()
                            .justify_center()
                            .when_some(self.source_previews.get(&index), |preview, image| {
                                preview.child(
                                    img(image.clone())
                                        .size_full()
                                        .object_fit(ObjectFit::Contain),
                                )
                            })
                            .when(!self.source_previews.contains_key(&index), |preview| {
                                preview.child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .items_center()
                                        .gap_2()
                                        .child(icon(path, 25., MUTED))
                                        .child(
                                            div().text_size(px(10.)).text_color(rgb(MUTED)).child(
                                                if self.source_preview_done.contains(&index) {
                                                    "Prévia indisponível"
                                                } else {
                                                    "Carregando…"
                                                },
                                            ),
                                        ),
                                )
                            })
                            .when(selected, |preview| {
                                preview.child(
                                    div()
                                        .absolute()
                                        .right_2()
                                        .top_2()
                                        .size(px(20.))
                                        .rounded_full()
                                        .bg(rgb(FG))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(icon("icons/check.svg", 13., SUNK)),
                                )
                            });
                        line = line.child(
                            div()
                                .id(("source", index))
                                .flex_1()
                                .min_w_0()
                                .min_h_0()
                                .h_full()
                                .flex()
                                .flex_col()
                                .rounded(px(9.))
                                .border_1()
                                .border_color(rgb(if selected { FG } else { LINE }))
                                .bg(rgb(if selected { PANEL } else { SUNK }))
                                .overflow_hidden()
                                .cursor_pointer()
                                .hover(|card| card.bg(rgb(PANEL)))
                                .when(disabled, |card| card.opacity(0.5).cursor_default())
                                .child(preview)
                                .child(
                                    div()
                                        .h(px(if compact { 39. } else { 44. }))
                                        .flex_shrink_0()
                                        .min_w_0()
                                        .px_3()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(icon(path, 15., MUTED))
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .text_size(px(12.))
                                                .truncate()
                                                .child(source.name.clone()),
                                        ),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if this.starting || this.loading_sources {
                                        return;
                                    }
                                    this.selected_source = this.sources.iter().position(|source| {
                                        source.id == source_id && source.kind == kind
                                    });
                                    this.update_audio_for_source();
                                    cx.notify();
                                })),
                        );
                    } else {
                        line = line.child(div().flex_1().min_w_0());
                    }
                }
                rows = rows.child(line);
            }
            sources = sources.child(rows);
        }
        if !cfg!(target_os = "macos") {
            let count = choices.len();
            let label = if count == 0 {
                "0 fontes".into()
            } else {
                format!(
                    "{}–{} de {} {}",
                    page * page_size + 1,
                    ((page + 1) * page_size).min(count),
                    count,
                    if kind == Kind::Screen {
                        "telas"
                    } else {
                        "janelas"
                    }
                )
            };
            sources = sources.child(
                div()
                    .h(px(28.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_size(px(10.)).text_color(rgb(MUTED)).child(label))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(self.source_page_button(false, page, pages, cx))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(MUTED))
                                    .child(format!("{} / {pages}", page + 1)),
                            )
                            .child(self.source_page_button(true, page, pages, cx)),
                    ),
            );
        }

        let mut settings = div()
            .w(px(side_width))
            .flex_shrink_0()
            .min_h_0()
            .min_w_0()
            .border_l_1()
            .border_color(rgb(LINE))
            .pl(px(if compact { 18. } else { 22. }))
            .flex()
            .flex_col()
            .gap(px(if compact { 11. } else { 16. }));
        if cfg!(target_os = "windows") {
            settings = settings.child(self.audio_options(compact, cx));
        } else {
            settings = settings.child(div().min_w_0().flex().flex_col().gap_2().text_color(rgb(MUTED))
                .child(div().flex().items_center().gap_2().child(icon("icons/mute.svg", 16., MUTED))
                    .child(div().text_size(px(12.)).child("Compartilhamento sem áudio")))
                .child(div().text_size(px(11.)).line_height(px(16.)).whitespace_normal()
                    .child("Esta versão do Mac envia apenas vídeo. Você continua ouvindo as transmissões recebidas.")));
        }
        settings = settings.child(div().h(px(1.)).flex_shrink_0().bg(rgb(LINE)));
        let mut quality_choices = div().flex_shrink_0().flex().flex_col().gap(px(7.)).child(
            div()
                .mb_1()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .child("Qualidade da transmissão"),
        );
        for (index, (quality, note)) in [
            (Quality::FullHd60, "Mais fluidez"),
            (Quality::FullHd30, "Menos uso do PC"),
            (Quality::Hd60, "Menos banda"),
        ]
        .into_iter()
        .enumerate()
        {
            let selected = self.quality == quality;
            quality_choices = quality_choices.child(
                div()
                    .id(("quality", index))
                    .h(px(if compact { 44. } else { 52. }))
                    .flex_shrink_0()
                    .px_3()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(rgb(if selected { FG } else { LINE }))
                    .bg(rgb(if selected { PANEL } else { BG }))
                    .flex()
                    .items_center()
                    .gap_3()
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(PANEL)))
                    .when(self.starting || self.publishing.is_some(), |option| {
                        option.opacity(0.45).cursor_default()
                    })
                    .child(
                        div()
                            .size(px(13.))
                            .flex_shrink_0()
                            .rounded_full()
                            .border_1()
                            .border_color(rgb(if selected { FG } else { MUTED }))
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(selected, |circle| {
                                circle.child(div().size(px(5.)).rounded_full().bg(rgb(FG)))
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(div().text_size(px(12.)).child(quality.label()))
                            .child(div().text_size(px(10.)).text_color(rgb(MUTED)).child(note)),
                    )
                    .when(selected, |option| {
                        option.child(icon("icons/check.svg", 13., FG))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.starting && this.publishing.is_none() {
                            this.quality = quality;
                            cx.notify();
                        }
                    })),
            );
        }
        settings = settings.child(quality_choices);
        let can_start = !disabled && self.selected_source.is_some() && self.audio_selection_ready();
        let selected = self.selected_source.and_then(|i| self.sources.get(i));
        let audio_label = if self.audio_selection == audio_selection::Selection::Off
            || !cfg!(target_os = "windows")
        {
            "sem áudio".to_owned()
        } else if let Some(application) = self.selected_audio_application() {
            format!("áudio de {}", application.name)
        } else {
            "escolha o áudio".to_owned()
        };
        let footer = div()
            .h(px(if compact { 76. } else { 82. }))
            .flex_shrink_0()
            .px(px(padding))
            .border_t_1()
            .border_color(rgb(LINE))
            .flex()
            .items_center()
            .gap_4()
            .child(icon(
                if selected.is_some_and(|s| s.kind == Kind::Screen) {
                    "icons/monitor.svg"
                } else {
                    "icons/window.svg"
                },
                20.,
                MUTED,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .truncate()
                            .child(
                                selected
                                    .map(|s| s.name.clone())
                                    .unwrap_or_else(|| "Selecione uma fonte".into()),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(MUTED))
                            .truncate()
                            .child(format!("{} · {audio_label}", self.quality.label())),
                    ),
            )
            .child(
                quiet_button("cancel-picker", "Cancelar").on_click(cx.listener(
                    |this, _, _, cx| {
                        if this.starting {
                            let _ = this.backend.commands.send(Command::CancelShare);
                            this.starting = false;
                        }
                        this.close_source_picker(cx);
                        cx.notify();
                    },
                )),
            )
            .child(
                button_with_icon(
                    "start",
                    if self.starting {
                        "Preparando…"
                    } else if self.publishing.is_some() {
                        "Trocar tela"
                    } else {
                        "Começar a transmitir"
                    },
                    "icons/share.svg",
                    true,
                )
                .h(px(42.))
                .text_size(px(12.))
                .when(!can_start, |button| button.opacity(0.45))
                .on_click(cx.listener(|this, _, _, cx| this.share(cx))),
            );
        let mut picker = div()
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .child(title)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .px(px(padding))
                    .pb(px(if compact { 16. } else { 20. }))
                    .flex()
                    .gap(px(if compact { 20. } else { 24. }))
                    .child(sources)
                    .child(settings),
            )
            .child(footer);
        if self.audio_apps_open && cfg!(target_os = "windows") {
            let menu = self.audio_app_menu(
                compact,
                side_width + padding + 12.,
                if compact { 82. } else { 102. },
                cx,
            );
            picker = picker.child(deferred(menu).with_priority(3));
        }
        picker.into_any_element()
    }

    fn source_page_button(
        &self,
        next: bool,
        page: usize,
        pages: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let disabled = self.starting
            || self.loading_sources
            || if next { page + 1 >= pages } else { page == 0 };
        div()
            .id(("source-page", usize::from(next)))
            .size(px(28.))
            .rounded(px(6.))
            .border_1()
            .border_color(rgb(LINE))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .when(disabled, |button| button.opacity(0.3).cursor_default())
            .child(icon(
                if next {
                    "icons/next.svg"
                } else {
                    "icons/previous.svg"
                },
                14.,
                MUTED,
            ))
            .on_click(cx.listener(move |this, _, _, cx| {
                if !disabled {
                    this.source_page = if next { page + 1 } else { page - 1 };
                    cx.notify();
                }
            }))
            .into_any_element()
    }

    fn audio_options(&self, compact: bool, cx: &mut Context<Self>) -> AnyElement {
        use audio_selection::Selection;
        let disabled = self.starting || self.loading_sources;
        let can_follow = self
            .selected_source
            .and_then(|i| self.sources.get(i))
            .is_some_and(|source| {
                source.supports_audio()
                    && self
                        .audio_sources
                        .iter()
                        .any(|app| Some(app.window_id) == source.id)
            });
        let mut modes = div()
            .h(px(34.))
            .flex_shrink_0()
            .p(px(3.))
            .bg(rgb(SUNK))
            .rounded(px(7.))
            .border_1()
            .border_color(rgb(LINE))
            .flex()
            .gap(px(2.));
        for (index, label) in ["Da janela", "Outro app", "Sem áudio"]
            .into_iter()
            .enumerate()
        {
            let selected = matches!(
                (index, self.audio_selection),
                (0, Selection::FollowWindow) | (1, Selection::Application(_)) | (2, Selection::Off)
            );
            let unavailable = disabled || (index == 0 && !can_follow);
            modes = modes.child(
                div()
                    .id(("audio-mode", index))
                    .flex_1()
                    .h_full()
                    .rounded(px(4.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(10.))
                    .cursor_pointer()
                    .text_color(rgb(if selected { FG } else { MUTED }))
                    .when(selected, |button| button.bg(rgb(PANEL)))
                    .when(unavailable, |button| button.opacity(0.4).cursor_default())
                    .child(label)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if unavailable {
                            return;
                        }
                        this.audio_selection_warning = None;
                        this.audio_app_page = 0;
                        this.audio_search.clear();
                        match index {
                            0 => {
                                this.audio_selection = Selection::FollowWindow;
                                this.audio_apps_open = false;
                            }
                            1 => {
                                if !matches!(this.audio_selection, Selection::Application(_)) {
                                    this.audio_selection = Selection::Application(None);
                                }
                                this.audio_apps_open = true;
                            }
                            _ => {
                                this.audio_selection = Selection::Off;
                                this.audio_apps_open = false;
                            }
                        }
                        cx.notify();
                    })),
            );
        }
        let other = matches!(self.audio_selection, Selection::Application(_));
        let title = if self.audio_selection == Selection::Off {
            "Compartilhar sem som".into()
        } else {
            self.selected_audio_application()
                .map(|a| a.name.clone())
                .unwrap_or_else(|| "Escolher aplicativo".into())
        };
        let note = if self.audio_selection == Selection::Off {
            "A voz continua no Discord"
        } else if other {
            "Somente deste aplicativo"
        } else {
            "Acompanha a janela escolhida"
        };
        let help = self
            .audio_selection_warning
            .unwrap_or(match self.audio_selection {
                Selection::Off => "Só a imagem será transmitida.",
                Selection::FollowWindow => "Seus amigos ouvem só a janela escolhida.",
                Selection::Application(_) => "Discord e Telinha ficam fora das fontes de áudio.",
            });
        div()
            .flex_shrink_0()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(9.))
            .child(
                div()
                    .h(px(20.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Fonte de áudio"),
                    )
                    .child(icon("icons/volume.svg", 15., MUTED)),
            )
            .child(modes)
            .child(
                div()
                    .id("audio-selected-app")
                    .h(px(if compact { 45. } else { 53. }))
                    .flex_shrink_0()
                    .px_3()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(rgb(if self.audio_apps_open { FG } else { LINE }))
                    .bg(rgb(SUNK))
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .when(other && !disabled, |button| {
                        button.cursor_pointer().hover(|style| style.bg(rgb(PANEL)))
                    })
                    .child(icon(
                        if self.audio_selection == Selection::Off {
                            "icons/mute.svg"
                        } else {
                            "icons/window.svg"
                        },
                        16.,
                        MUTED,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .child(div().text_size(px(11.)).truncate().child(title))
                            .child(
                                div()
                                    .text_size(px(9.))
                                    .text_color(rgb(MUTED))
                                    .truncate()
                                    .child(note),
                            ),
                    )
                    .when(other, |button| {
                        button.child(icon("icons/chevron.svg", 12., MUTED))
                    })
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if other && !disabled {
                            this.audio_apps_open = !this.audio_apps_open;
                            cx.notify();
                        }
                    })),
            )
            .child(
                div()
                    .h(px(30.))
                    .flex_shrink_0()
                    .min_w_0()
                    .text_size(px(10.))
                    .line_height(px(15.))
                    .whitespace_normal()
                    .text_color(rgb(if self.audio_selection_warning.is_some() {
                        WARN
                    } else {
                        MUTED
                    }))
                    .child(help),
            )
            .into_any_element()
    }

    fn audio_app_menu(
        &self,
        compact: bool,
        right: f32,
        top: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use audio_selection::Selection;
        let query = self.audio_search.to_lowercase();
        let choices: Vec<_> = self
            .audio_sources
            .iter()
            .enumerate()
            .filter(|(_, app)| app.name.to_lowercase().contains(&query))
            .collect();
        let pages = choices.len().div_ceil(4).max(1);
        let page = self.audio_app_page.min(pages - 1);
        let mut applications = div()
            .h(px(if compact { 158. } else { 174. }))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(2.));
        if choices.is_empty() {
            applications =
                applications.child(div().p_3().text_size(px(11.)).text_color(rgb(MUTED)).child(
                    if self.audio_sources.is_empty() {
                        "Abra um aplicativo e atualize a lista."
                    } else {
                        "Nenhum aplicativo encontrado."
                    },
                ));
        }
        for (index, application) in choices.iter().skip(page * 4).take(4) {
            let key = audio_application_key(application);
            let selected = self.audio_selection == Selection::Application(Some(key));
            applications = applications.child(
                div()
                    .id(("audio-app", *index))
                    .h(px(if compact { 38. } else { 42. }))
                    .flex_shrink_0()
                    .min_w_0()
                    .px_2()
                    .rounded(px(6.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .when(selected, |item| item.bg(rgb(PANEL)))
                    .hover(|style| style.bg(rgb(PANEL)))
                    .child(icon("icons/window.svg", 15., MUTED))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(11.))
                            .truncate()
                            .child(application.name.clone()),
                    )
                    .when(selected, |item| {
                        item.child(icon("icons/check.svg", 13., FG))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.starting || this.loading_sources {
                            return;
                        }
                        if this
                            .audio_sources
                            .iter()
                            .any(|app| audio_application_key(app) == key)
                        {
                            this.audio_selection = Selection::Application(Some(key));
                            this.audio_selection_warning = None;
                            this.audio_apps_open = false;
                            cx.notify();
                        }
                    })),
            );
        }
        let mut pagination = div()
            .h(px(26.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(rgb(MUTED))
                    .child(format!("{} aplicativos", choices.len())),
            );
        let mut arrows = div().flex().items_center().gap_2();
        for next in [false, true] {
            let disabled = if next { page + 1 >= pages } else { page == 0 };
            arrows = arrows.child(
                div()
                    .id(("audio-page", usize::from(next)))
                    .size(px(26.))
                    .rounded(px(5.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .when(disabled, |button| button.opacity(0.3).cursor_default())
                    .child(icon(
                        if next {
                            "icons/next.svg"
                        } else {
                            "icons/previous.svg"
                        },
                        13.,
                        MUTED,
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !disabled {
                            this.audio_app_page = if next { page + 1 } else { page - 1 };
                            cx.notify();
                        }
                    })),
            );
        }
        pagination = pagination.child(arrows);
        div()
            .id("audio-app-popover")
            .absolute()
            .right(px(right))
            .top(px(top))
            .w(px(if compact { 278. } else { 294. }))
            .p(px(10.))
            .rounded(px(10.))
            .border_1()
            .border_color(rgb(0x68707B))
            .bg(rgb(0x292D32))
            .shadow_lg()
            .flex()
            .flex_col()
            .gap(px(8.))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Qual aplicativo vamos ouvir?"),
                    )
                    .child(
                        div()
                            .id("close-audio-apps")
                            .size(px(24.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .child(icon("icons/close.svg", 14., MUTED))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.audio_apps_open = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .h(px(32.))
                    .flex_shrink_0()
                    .px_2()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(rgb(LINE))
                    .bg(rgb(SUNK))
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .child(icon("icons/search.svg", 14., MUTED))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(11.))
                            .truncate()
                            .text_color(rgb(if self.audio_search.is_empty() {
                                MUTED
                            } else {
                                FG
                            }))
                            .child(if self.audio_search.is_empty() {
                                "Buscar aplicativo".into()
                            } else {
                                self.audio_search.clone()
                            }),
                    ),
            )
            .child(applications)
            .child(pagination)
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(rgb(MUTED))
                    .child("Discord e Telinha ficam fora da lista."),
            )
            .into_any_element()
    }
}
