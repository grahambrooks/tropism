import _ from 'lodash';
import { format } from './format.js';

export function helper() {
  return _.chunk(format([1, 2]), 1);
}
