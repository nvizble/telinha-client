use super::*;

impl Telinha {
    pub(super) fn sync_preview_enabled(&self) {
        self.backend
            .media
            .preview
            .set_enabled(self.publishing.is_some() && (self.local_preview || self.gallery));
    }

    pub(super) fn drop_screen_image(&mut self, id: &str, cx: &mut Context<Self>) {
        self.backend.media.screens.lock().remove(id);
        if let Some(image) = self.screen_images.remove(id) {
            cx.drop_image(image, None);
        }
    }

    pub(super) fn set_gallery(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.gallery = enabled;
        self.backend.media.gallery.store(enabled, Ordering::Relaxed);
        if enabled && self.local_preview {
            self.local_preview = false;
            self.sync_stream_selection(cx);
        }
        self.sync_camera_focus();
        self.sync_preview_enabled();
        if !enabled {
            self.backend.media.screens.lock().clear();
            for (_, image) in self.screen_images.drain() {
                cx.drop_image(image, None);
            }
        }
        self.controls_visible = true;
        self.ghost_visible = true;
        self.mouse_at = Instant::now();
        cx.notify();
    }

    fn gallery_screen_tile(
        &self,
        stream: Option<&RemoteStream>,
        height: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = stream.map(|s| s.id.clone());
        let active = id
            .as_deref()
            .is_some_and(|id| self.backend.media.selected_track.lock().as_deref() == Some(id));
        let title = stream
            .map(|s| s.title.clone())
            .unwrap_or_else(|| "Você · sua tela".into());
        let image = id
            .as_ref()
            .and_then(|id| self.screen_images.get(id))
            .or_else(|| {
                if id.is_none() {
                    self.local_preview_image.as_ref()
                } else {
                    None
                }
            });
        let mut tile = div()
            .id(SharedString::from(format!(
                "gallery-screen-{}",
                id.as_deref().unwrap_or("own")
            )))
            .relative()
            .min_w_0()
            .h(px(height))
            .overflow_hidden()
            .rounded(px(10.))
            .border_1()
            .border_color(rgb(if active { FG } else { LINE }))
            .bg(rgb(0x17181A))
            .cursor_pointer()
            .hover(|s| s.border_color(rgb(FG)))
            .on_click(cx.listener({
                let id = id.clone();
                move |this, _, _, cx| {
                    if let Some(id) = &id {
                        this.select_stream(Some(id.clone()), cx);
                    } else {
                        this.set_local_preview(true, cx);
                    }
                }
            }));
        tile = if let Some(image) = image {
            tile.child(
                img(image.clone())
                    .size_full()
                    .object_fit(ObjectFit::Contain),
            )
        } else {
            tile.child(
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon("icons/monitor.svg", 28., MUTED)),
            )
        };
        tile.child(
            div()
                .absolute()
                .bottom_0()
                .left_0()
                .right_0()
                .h(px(38.))
                .px_3()
                .bg(gpui::rgba(0x17181AE6))
                .flex()
                .items_center()
                .gap_2()
                .child(icon(
                    if active {
                        "icons/volume.svg"
                    } else if id.is_none() {
                        "icons/mute.svg"
                    } else {
                        "icons/monitor.svg"
                    },
                    15.,
                    if active { FG } else { MUTED },
                ))
                .child(div().flex_1().min_w_0().truncate().text_sm().child(title))
                .child(
                    button_with_icon(
                        SharedString::from(format!(
                            "gallery-expand-{}",
                            id.as_deref().unwrap_or("own")
                        )),
                        "",
                        "icons/fullscreen.svg",
                        false,
                    )
                    .size(px(30.))
                    .px_0()
                    .bg(gpui::rgba(0))
                    .border_color(gpui::rgba(0))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        if let Some(id) = &id {
                            this.select_stream(Some(id.clone()), cx);
                        } else {
                            this.set_local_preview(true, cx);
                        }
                        if !this.fullscreen {
                            window.toggle_fullscreen();
                            this.fullscreen = true;
                        }
                        cx.notify();
                    })),
                ),
        )
        .into_any_element()
    }

    pub(super) fn gallery_view(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let count =
            self.streams.len() + self.camera_count() + usize::from(self.publishing.is_some());
        if count == 0 {
            return session_view::empty_state(
                "icons/gallery.svg",
                "Ninguém está transmitindo",
                "As telas e webcams aparecem juntas aqui.",
            )
            .into_any_element();
        }
        let width = f32::from(window.viewport_size().width);
        let columns = if count == 1 {
            1
        } else if count > 4 && width >= 1000. {
            3
        } else {
            2
        };
        let rows = count.div_ceil(columns);
        let available =
            f32::from(window.viewport_size().height) - if self.fullscreen { 90. } else { 160. };
        let height = (available / rows as f32 - 12.).clamp(140., 540.);
        let mut grid = div()
            .id("video-gallery")
            .size_full()
            .min_h_0()
            .min_w_0()
            .overflow_y_scroll()
            .p_3()
            .grid()
            .grid_cols(columns as u16)
            .gap_3();
        for stream in &self.streams {
            grid = grid.child(self.gallery_screen_tile(Some(stream), height, cx));
        }
        if self.publishing.is_some() {
            grid = grid.child(self.gallery_screen_tile(None, height, cx));
        }
        grid.children(self.camera_tiles(height, cx))
            .into_any_element()
    }
}
