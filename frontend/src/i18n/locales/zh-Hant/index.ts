import accounts from './accounts.json'
import appearance from './appearance.json'
import auth from './authentication.json'
import common from './common.json'
import dashboard from './dashboard.json'
import groups from './groups.json'
import keys from './keys.json'
import navigation from './navigation.json'
import proxies from './proxies.json'
import settings from './settings.json'
import systemUpdate from './systemUpdate.json'
import ui from './ui.json'
import usage from './usage.json'

export default {
  ...ui,
  ...accounts,
  ...appearance,
  ...auth,
  ...common,
  ...dashboard,
  ...groups,
  ...keys,
  ...navigation,
  ...proxies,
  ...settings,
  ...systemUpdate,
  ...usage,
}
