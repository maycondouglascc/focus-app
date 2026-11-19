import Gio from 'gi://Gio';
import Shell from 'gi://Shell';
import { Extension } from 'resource:///org/gnome/shell/extensions/extension.js';

const BUS_NAME = 'org.focus.WindowTracker';
const OBJECT_PATH = '/org/focus/WindowTracker';

const IFACE_XML = `
<node>
  <interface name="${BUS_NAME}">
    <method name="GetFocusedWindow">
      <arg type="s" direction="out" name="appId"/>
      <arg type="s" direction="out" name="wmClass"/>
      <arg type="s" direction="out" name="title"/>
      <arg type="u" direction="out" name="pid"/>
    </method>
  </interface>
</node>`;

export default class FocusWindowTrackerExtension extends Extension {
  enable() {
    this._dbusObject = Gio.DBusExportedObject.wrapJSObject(IFACE_XML, this);
    this._dbusObject.export(Gio.DBus.session, OBJECT_PATH);
    this._ownName = Gio.DBus.session.own_name(BUS_NAME, Gio.BusNameOwnerFlags.NONE, null, null);
  }

  disable() {
    if (this._ownName) {
      Gio.DBus.session.unown_name(this._ownName);
      this._ownName = 0;
    }
    if (this._dbusObject) {
      try {
        this._dbusObject.unexport();
      } catch {
        /* ignore if already unexported */
      }
      this._dbusObject.run_dispose();
      this._dbusObject = null;
    }
  }

  GetFocusedWindow() {
    const win = global.display.focus_window;
    if (!win) return ['', '', '', 0];
    const app = Shell.WindowTracker.get_default().get_window_app(win);
    const appId = app ? app.get_id() || '' : '';
    const wmClass = win.get_wm_class() || '';
    const title = win.get_title() || '';
    const pid = Math.max(0, win.get_pid() || 0);
    return [appId, wmClass, title, pid];
  }
}
