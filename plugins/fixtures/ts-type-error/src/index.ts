// A type error the build should refuse: the rest of the plugin is fine.
import { definePlugin, log } from 'rekindle';

const count: number = 'three';

export default definePlugin({
  activate() {
    log.info(`count is ${count}`);
  },
});
