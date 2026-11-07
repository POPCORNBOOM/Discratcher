import { createPinia } from 'pinia'
import { createApp } from 'vue'
import App from './App.vue'
import { setupGlobalTooltip } from './tooltip'
import './styles/tokens.css'
import './styles/base.css'

createApp(App).use(createPinia()).mount('#app')
setupGlobalTooltip()
