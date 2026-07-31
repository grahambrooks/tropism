import chalk from 'chalk';
import { helper } from './helper.js';

export function format(x) {
  return chalk.green(String(x)) + helper.name;
}
