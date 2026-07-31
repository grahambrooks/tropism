import express from 'express';
import { helper } from './utils/helper.js';
import fs from 'node:fs';

export function start() {
  fs.existsSync('.');
  return express() && helper();
}
