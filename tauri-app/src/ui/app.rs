use leptos::prelude::*;
use crate::ui::lab::LabView;
use crate::ui::teacher::TeacherView;
use crate::ui::settings::SettingsView;

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Lab,
    Teacher,
    Settings,
}

#[component]
pub fn App() -> impl IntoView {
    let (active_tab, set_active_tab) = signal(Tab::Lab);

    view! {
        <div class="app-container">
            <nav class="tab-strip">
                <button
                    class=move || if active_tab.get() == Tab::Lab { "active" } else { "" }
                    on:click=move |_| set_active_tab.set(Tab::Lab)
                >
                    "Voice Lab"
                </button>
                <button
                    class=move || if active_tab.get() == Tab::Teacher { "active" } else { "" }
                    on:click=move |_| set_active_tab.set(Tab::Teacher)
                >
                    "Teacher"
                </button>
                <button
                    class=move || if active_tab.get() == Tab::Settings { "active" } else { "" }
                    on:click=move |_| set_active_tab.set(Tab::Settings)
                >
                    "Settings"
                </button>
            </nav>
            <main class="content">
                <Show when=move || active_tab.get() == Tab::Lab>
                    <LabView />
                </Show>
                <Show when=move || active_tab.get() == Tab::Teacher>
                    <TeacherView />
                </Show>
                <Show when=move || active_tab.get() == Tab::Settings>
                    <SettingsView />
                </Show>
            </main>
        </div>
    }
}
