import { createApp } from 'vue'
import './style.css'
import App from './App.vue'
import router from './router'
import './api' // side-effect: register axios interceptors

const app = createApp(App)
app.use(router)
app.mount('#app')
