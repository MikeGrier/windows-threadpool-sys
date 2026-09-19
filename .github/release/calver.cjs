// Copyright (c) Mike Grier.
'use strict';

function nextCalendarVersion(previous, date = new Date()) {
  if (!(date instanceof Date) || !Number.isFinite(date.valueOf())) throw new Error('Invalid release date');
  const match = /^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/.exec(previous);
  if (!match) throw new Error('Calendar version must have three numeric components');
  const [year, day, counter] = match.slice(1).map(Number);
  if (![year, day, counter].every(Number.isSafeInteger)) throw new Error('Version exceeds safe integer range');
  if (year !== 0) {
    const month = Math.floor(day / 100);
    const monthDay = day % 100;
    const parsed = new Date(Date.UTC(year, month - 1, monthDay));
    if (year < 1000 || month < 1 || month > 12 || parsed.getUTCFullYear() !== year || parsed.getUTCMonth() + 1 !== month || parsed.getUTCDate() !== monthDay) {
      throw new Error('Invalid calendar date in previous version');
    }
  }
  const todayYear = date.getUTCFullYear();
  const todayDay = (date.getUTCMonth() + 1) * 100 + date.getUTCDate();
  if (todayYear < 1000) throw new Error('Release year must be at least 1000');
  if (todayYear > year || (todayYear === year && todayDay > day)) return `${todayYear}.${todayDay}.0`;
  if (!Number.isSafeInteger(counter + 1)) throw new Error('Calendar counter exhausted');
  return `${year}.${day}.${counter + 1}`;
}

module.exports = { nextCalendarVersion };