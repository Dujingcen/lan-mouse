mod imp;

use adw::prelude::*;
use adw::subclass::prelude::*;
use glib::{clone, Object};
use gtk::{
    gio,
    glib::{self, closure_local},
    ListBox, NoSelection,
};

use lan_mouse_ipc::{
    ClientConfig, ClientHandle, ClientState, FrontendRequest, FrontendRequestWriter, Position,
    DEFAULT_PORT,
};

use super::{client_object::ClientObject, client_row::ClientRow};

glib::wrapper! {
    pub struct Window(ObjectSubclass<imp::Window>)
        @extends adw::ApplicationWindow, gtk::Window, gtk::Widget,
        @implements gio::ActionGroup, gio::ActionMap, gtk::Accessible, gtk::Buildable,
                    gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl Window {
    pub(crate) fn new(app: &adw::Application, conn: FrontendRequestWriter) -> Self {
        let window: Self = Object::builder().property("application", app).build();
        window
            .imp()
            .frontend_request_writer
            .borrow_mut()
            .replace(conn);
        window
    }

    pub fn clients(&self) -> gio::ListStore {
        self.imp()
            .clients
            .borrow()
            .clone()
            .expect("Could not get clients")
    }

    fn client_by_idx(&self, idx: u32) -> Option<ClientObject> {
        self.clients().item(idx).map(|o| o.downcast().unwrap())
    }

    fn setup_clients(&self) {
        let model = gio::ListStore::new::<ClientObject>();
        self.imp().clients.replace(Some(model));

        let selection_model = NoSelection::new(Some(self.clients()));
        self.imp().client_list.bind_model(
            Some(&selection_model),
            clone!(
                #[weak(rename_to = window)]
                self,
                #[upgrade_or_panic]
                move |obj| {
                    let client_object = obj
                        .downcast_ref()
                        .expect("Expected object of type `ClientObject`.");
                    let row = window.create_client_row(client_object);
                    row.connect_closure(
                        "request-update",
                        false,
                        closure_local!(
                            #[strong]
                            window,
                            move |row: ClientRow, active: bool| {
                                if let Some(client) = window.client_by_idx(row.index() as u32) {
                                    window.request_client_activate(&client, active);
                                    window.request_client_update(&client);
                                    window.request_client_state(&client);
                                }
                            }
                        ),
                    );
                    row.connect_closure(
                        "request-delete",
                        false,
                        closure_local!(
                            #[strong]
                            window,
                            move |row: ClientRow| {
                                if let Some(client) = window.client_by_idx(row.index() as u32) {
                                    window.request_client_delete(&client);
                                }
                            }
                        ),
                    );
                    row.connect_closure(
                        "request-dns",
                        false,
                        closure_local!(
                            #[strong]
                            window,
                            move |row: ClientRow| {
                                if let Some(client) = window.client_by_idx(row.index() as u32) {
                                    window.request_client_update(&client);
                                    window.request_dns(&client);
                                    window.request_client_state(&client);
                                }
                            }
                        ),
                    );
                    row.upcast()
                }
            ),
        );
    }

    /// workaround for a bug in libadwaita that shows an ugly line beneath
    /// the last element if a placeholder is set.
    /// https://gitlab.gnome.org/GNOME/gtk/-/merge_requests/6308
    pub fn set_placeholder_visible(&self, visible: bool) {
        let placeholder = self.imp().client_placeholder.get();
        self.imp().client_list.set_placeholder(match visible {
            true => Some(&placeholder),
            false => None,
        });
    }

    fn setup_icon(&self) {
        self.set_icon_name(Some("de.feschber.LanMouse"));
    }

    fn create_client_row(&self, client_object: &ClientObject) -> ClientRow {
        let row = ClientRow::new(client_object);
        row.bind(client_object);
        row
    }

    pub fn new_client(&self, handle: ClientHandle, client: ClientConfig, state: ClientState) {
        let client = ClientObject::new(handle, client, state.clone());
        self.clients().append(&client);
        self.set_placeholder_visible(false);
        self.update_dns_state(handle, !state.ips.is_empty());
    }

    pub fn client_idx(&self, handle: ClientHandle) -> Option<usize> {
        self.clients().iter::<ClientObject>().position(|c| {
            if let Ok(c) = c {
                c.handle() == handle
            } else {
                false
            }
        })
    }

    pub fn delete_client(&self, handle: ClientHandle) {
        let Some(idx) = self.client_idx(handle) else {
            log::warn!("could not find client with handle {handle}");
            return;
        };

        self.clients().remove(idx as u32);
        if self.clients().n_items() == 0 {
            self.set_placeholder_visible(true);
        }
    }

    pub fn update_client_config(&self, handle: ClientHandle, client: ClientConfig) {
        let Some(idx) = self.client_idx(handle) else {
            log::warn!("could not find client with handle {}", handle);
            return;
        };
        let client_object = self.clients().item(idx as u32).unwrap();
        let client_object: &ClientObject = client_object.downcast_ref().unwrap();
        let data = client_object.get_data();

        /* only change if it actually has changed, otherwise
         * the update signal is triggered */
        if data.hostname != client.hostname {
            client_object.set_hostname(client.hostname.unwrap_or("".into()));
        }
        if data.port != client.port as u32 {
            client_object.set_port(client.port as u32);
        }
        if data.position != client.pos.to_string() {
            client_object.set_position(client.pos.to_string());
        }
    }

    pub fn update_client_state(&self, handle: ClientHandle, state: ClientState) {
        let Some(idx) = self.client_idx(handle) else {
            log::warn!("could not find client with handle {}", handle);
            return;
        };
        let client_object = self.clients().item(idx as u32).unwrap();
        let client_object: &ClientObject = client_object.downcast_ref().unwrap();
        let data = client_object.get_data();

        if state.active != data.active {
            client_object.set_active(state.active);
            log::debug!("set active to {}", state.active);
        }

        if state.resolving != data.resolving {
            client_object.set_resolving(state.resolving);
            log::debug!("resolving {}: {}", data.handle, state.resolving);
        }

        self.update_dns_state(handle, !state.ips.is_empty());
        let ips = state
            .ips
            .into_iter()
            .map(|ip| ip.to_string())
            .collect::<Vec<_>>();
        client_object.set_ips(ips);
    }

    pub fn update_dns_state(&self, handle: ClientHandle, resolved: bool) {
        let Some(idx) = self.client_idx(handle) else {
            log::warn!("could not find client with handle {}", handle);
            return;
        };
        let list_box: ListBox = self.imp().client_list.get();
        let row = list_box.row_at_index(idx as i32).unwrap();
        let client_row: ClientRow = row.downcast().expect("expected ClientRow Object");
        if resolved {
            client_row.imp().dns_button.set_css_classes(&["success"])
        } else {
            client_row.imp().dns_button.set_css_classes(&["warning"])
        }
    }

    pub fn request_port_change(&self) {
        let port = self
            .imp()
            .port_entry
            .get()
            .text()
            .as_str()
            .parse::<u16>()
            .unwrap_or(DEFAULT_PORT);
        self.request(FrontendRequest::ChangePort(port));
    }

    pub fn request_capture(&self) {
        self.request(FrontendRequest::EnableCapture);
    }

    pub fn request_emulation(&self) {
        self.request(FrontendRequest::EnableEmulation);
    }

    pub fn request_client_state(&self, client: &ClientObject) {
        self.request(FrontendRequest::GetState(client.handle()));
    }

    pub fn request_client_create(&self) {
        self.request(FrontendRequest::Create);
    }

    pub fn request_dns(&self, client: &ClientObject) {
        self.request(FrontendRequest::ResolveDns(client.get_data().handle));
    }

    pub fn request_client_update(&self, client: &ClientObject) {
        let handle = client.handle();
        let data = client.get_data();
        let position = Position::try_from(data.position.as_str()).expect("invalid position");
        let hostname = data.hostname;
        let port = data.port as u16;

        for event in [
            FrontendRequest::UpdateHostname(handle, hostname),
            FrontendRequest::UpdatePosition(handle, position),
            FrontendRequest::UpdatePort(handle, port),
        ] {
            self.request(event);
        }
    }

    pub fn request_client_activate(&self, client: &ClientObject, active: bool) {
        self.request(FrontendRequest::Activate(client.handle(), active));
    }

    pub fn request_client_delete(&self, client: &ClientObject) {
        self.request(FrontendRequest::Delete(client.handle()));
    }

    pub fn request(&self, request: FrontendRequest) {
        let mut requester = self.imp().frontend_request_writer.borrow_mut();
        let requester = requester.as_mut().unwrap();
        if let Err(e) = requester.request(request) {
            log::error!("error sending message: {e}");
        };
    }

    pub fn show_toast(&self, msg: &str) {
        let toast = adw::Toast::new(msg);
        let toast_overlay = &self.imp().toast_overlay;
        toast_overlay.add_toast(toast);
    }

    pub fn set_capture(&self, active: bool) {
        self.imp().capture_active.replace(active);
        self.update_capture_emulation_status();
    }

    pub fn set_emulation(&self, active: bool) {
        self.imp().emulation_active.replace(active);
        self.update_capture_emulation_status();
    }

    fn update_capture_emulation_status(&self) {
        let capture = self.imp().capture_active.get();
        let emulation = self.imp().emulation_active.get();
        self.imp().capture_status_row.set_visible(!capture);
        self.imp().emulation_status_row.set_visible(!emulation);
        self.imp()
            .capture_emulation_group
            .set_visible(!capture || !emulation);
    }
}