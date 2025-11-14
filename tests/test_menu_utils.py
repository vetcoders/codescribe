import rumps

from vistascribe.app.menu_utils import create_parent_item, ensure_parent_callback, set_submenu


def test_parent_menu_items_remain_enabled():
    parent = create_parent_item("Sample Parent")
    child = rumps.MenuItem("Child", callback=lambda _s: None)

    # Initial state should already provide a callback
    assert parent.callback is not None

    set_submenu(parent, [child])
    ensure_parent_callback(parent)

    assert parent.callback is not None
    assert any(entry.title == "Child" for entry in parent.menu)
