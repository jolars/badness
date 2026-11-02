import DefaultTheme from "vitepress/theme";
import { useData, useRoute } from "vitepress";
import {
  defineComponent,
  h,
  nextTick,
  onMounted,
  onUnmounted,
  watch,
} from "vue";
import { mountCharts, unmountCharts } from "./bench-charts.js";
import "./bench-charts.css";
import "./custom.css";

export default {
  extends: DefaultTheme,
  Layout: defineComponent({
    setup() {
      const route = useRoute();
      const { isDark } = useData();
      async function refresh() {
        unmountCharts();
        await nextTick();
        await mountCharts();
      }
      onMounted(refresh);
      watch([() => route.path, isDark], refresh, { flush: "post" });
      onUnmounted(unmountCharts);
      return () =>
        h(DefaultTheme.Layout, null, {
          "home-hero-before": () =>
            h("span", { id: "badness", "aria-hidden": "true" }),
        });
    },
  }),
};
