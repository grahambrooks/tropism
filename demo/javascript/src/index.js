import express from 'express';
import fs from 'node:fs';

import { helper } from './utils/helper.js';

export function start() {
  fs.existsSync('.');
  return express() && helper();
}
