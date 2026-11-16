// Reshaped's PostCSS config — resolves the @custom-media in reshaped/bundle.css
// (postcss-custom-media + global-data pointing at the slate theme's media.css) and
// minifies via cssnano. Required for Reshaped's responsive styles to work.
module.exports = require('reshaped/config/postcss').config;
