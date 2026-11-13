use super::*;

impl Telinha {
    pub(super) fn choose_camera(&mut self, cx: &mut Context<Self>) {
        self.close_source_picker(cx);
        self.camera_request += 1;
        self.camera_picker = true;
        self.camera_loading = true;
        self.camera_devices.clear();
        self.camera_selected = None;
        self.camera_error = None;
        let _ = self.backend.commands.send(Command::Cameras {
            generation: self.generation,
            request: self.camera_request,
        });
        cx.notify();
    }

    fn start_camera(&mut self, cx: &mut Context<Self>) {
        if self.camera_starting || self.camera_loading {
            return;
        }
        if let Some(device) = self
            .camera_selected
            .and_then(|index| self.camera_devices.get(index))
            .cloned()
        {
            self.camera_request += 1;
            self.camera_starting = true;
            self.camera_error = None;
            let _ = self.backend.commands.send(Command::StartCamera {
                request: self.camera_request,
                device,
            });
            cx.notify();
        }
    }

    pub(super) fn stop_camera(&mut self, cx: &mut Context<Self>) {
        self.camera_request += 1;
        self.camera_starting = false;
        self.camera_picker = false;
        self.camera_loading = false;
        self.clear_own_camera(cx);
        let _ = self.backend.commands.send(Command::StopCamera {
            request: self.camera_request,
        });
        cx.notify();
    }

    pub(super) fn drop_camera_image(&mut self, id: &str, cx: &mut Context<Self>) {
        self.backend.media.cameras.lock().remove(id);
        if let Some(image) = self.camera_images.remove(id) {
            cx.drop_image(image, None);
        }
    }

    pub(super) fn clear_own_camera(&mut self, cx: &mut Context<Self>) {
        if let Some((id, _)) = self.camera_publishing.take() {
            self.drop_camera_image(&id, cx);
        }
    }

    pub(super) fn reset_cameras(&mut self, cx: &mut Context<Self>) {
        self.camera_request += 1;
        self.camera_picker = false;
        self.camera_starting = false;
        self.camera_loading = false;
        self.camera_error = None;
        self.camera_publishing = None;
        self.cameras.clear();
        self.backend.media.cameras.lock().clear();
        for (_, image) in self.camera_images.drain() {
            cx.drop_image(image, None);
        }
    }

    pub(super) fn camera_count(&self) -> usize {
        self.cameras.len() + usize::from(self.camera_publishing.is_some())
    }

    fn camera_tile(
        &self,
        id: &str,
        name: &str,
        avatar: Option<&str>,
        own: bool,
        height: f32,
    ) -> AnyElement {
        div()
            .relative()
            .w_full()
            .min_w_0()
            .h(px(height))
            .flex_shrink_0()
            .rounded(px(10.))
            .overflow_hidden()
            .bg(rgb(SUNK))
            .border_1()
            .border_color(rgb(LINE))
            .flex()
            .items_center()
            .justify_center()
            .map(|tile| {
                if let Some(image) = self.camera_images.get(id) {
                    tile.child(
                        img(image.clone())
                            .size_full()
                            .object_fit(ObjectFit::Contain),
                    )
                } else {
                    tile.child(self.avatar(avatar, name, 48.))
                }
            })
            .child(
                div()
                    .absolute()
                    .left_2()
                    .right_2()
                    .bottom_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .py_1()
                            .min_w_0()
                            .rounded(px(5.))
                            .bg(gpui::rgba(0x17181ACC))
                            .child(icon("icons/camera.svg", 12., if own { LIVE } else { FG }))
                            .child(div().min_w_0().truncate().text_xs().child(if own {
                                "Você".to_owned()
                            } else {
                                name.to_owned()
                            })),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn camera_tiles(&self, height: f32) -> Vec<AnyElement> {
        let mut tiles: Vec<_> = self
            .cameras
            .iter()
            .map(|camera| {
                self.camera_tile(
                    &camera.id,
                    &camera.title,
                    camera.avatar_url.as_deref(),
                    false,
                    height,
                )
            })
            .collect();
        if let Some((id, _)) = &self.camera_publishing {
            tiles.push(self.camera_tile(
                id,
                "Você",
                self.account.as_ref().map(|a| a.avatar_url.as_str()),
                true,
                height,
            ));
        }
        tiles
    }

    pub(super) fn camera_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let can_start =
            !self.camera_loading && !self.camera_starting && self.camera_selected.is_some();
        let mut list = div()
            .id("camera-devices")
            .w_full()
            .min_w_0()
            .min_h_0()
            .flex_1()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_3();
        if self.camera_loading {
            list = list.child(
                div()
                    .p_5()
                    .rounded(px(10.))
                    .bg(rgb(SUNK))
                    .text_color(rgb(MUTED))
                    .whitespace_normal()
                    .child("Procurando webcams… Se o sistema pedir, permita o acesso à câmera."),
            );
        } else if let Some(message) = &self.camera_error {
            list = list.child(
                div()
                    .p_4()
                    .rounded(px(10.))
                    .bg(rgb(SUNK))
                    .text_color(rgb(WARN))
                    .whitespace_normal()
                    .child(message.clone()),
            );
        } else if self.camera_devices.is_empty() {
            list = list.child(
                div()
                    .p_5()
                    .rounded(px(10.))
                    .bg(rgb(SUNK))
                    .text_color(rgb(MUTED))
                    .whitespace_normal()
                    .child("Nenhuma webcam encontrada. Conecte uma câmera e atualize a lista."),
            );
        }
        for (index, device) in self.camera_devices.iter().enumerate() {
            let selected = self.camera_selected == Some(index);
            list = list.child(
                div()
                    .id(("camera-device", index))
                    .w_full()
                    .min_w_0()
                    .p_4()
                    .rounded(px(10.))
                    .border_1()
                    .border_color(rgb(if selected { FG } else { LINE }))
                    .bg(rgb(if selected { PANEL } else { SUNK }))
                    .flex()
                    .items_center()
                    .gap_3()
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(PANEL)))
                    .when(self.camera_starting, |card| {
                        card.opacity(0.5).cursor_default()
                    })
                    .child(icon("icons/camera.svg", 24., FG))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_normal()
                            .child(device.name.clone()),
                    )
                    .when(selected, |card| {
                        card.child(icon("icons/check.svg", 18., FG))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.camera_starting {
                            this.camera_selected = Some(index);
                            cx.notify();
                        }
                    })),
            );
        }
        div().flex_1().min_h_0().px_6().pb_5().flex().flex_col().items_center()
            .child(div().w_full().max_w(px(640.)).min_h_0().flex_1().flex().flex_col().gap_5()
                .child(div().flex().items_center().gap_4()
                    .child(div().flex_1().min_w_0().flex().flex_col().gap_2()
                        .child(div().text_size(px(24.)).font_weight(FontWeight::SEMIBOLD).child("Qual webcam você quer ligar?"))
                        .child(div().text_sm().text_color(rgb(MUTED)).whitespace_normal().child("Sua câmera aparece junto da tela. A voz continua na call do Discord.")))
                    .child(button_with_icon("refresh-cameras", "Atualizar", "icons/refresh.svg", false)
                        .when(self.camera_loading || self.camera_starting, |button| button.opacity(0.4))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if !this.camera_loading && !this.camera_starting { this.choose_camera(cx); }
                        }))))
                .child(list)
                .child(div().flex().items_center().gap_3().flex_shrink_0()
                    .child(div().flex_1().min_w_0().text_xs().line_height(px(18.)).whitespace_normal().text_color(rgb(MUTED))
                        .child("Ao ligar, todos na sessão podem ver você. Você pode desligar a webcam a qualquer momento."))
                    .child(quiet_button("cancel-camera", "Cancelar").on_click(cx.listener(|this, _, _, cx| {
                        this.stop_camera(cx);
                    })))
                    .child(button_with_icon("start-camera", if self.camera_starting { "Ligando…" } else { "Ligar webcam" }, "icons/camera.svg", true)
                        .when(!can_start, |button| button.opacity(0.45))
                        .on_click(cx.listener(|this, _, _, cx| this.start_camera(cx))))))
            .into_any_element()
    }
}
