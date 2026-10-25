"""Keyboard codes used by SDL2 and Qt config files."""
import string

SDL = {8: 'backspace', 9: 'tab', 13: 'enter', 27: 'escape', 32: 'space', 127: 'delete'}
SDL[1073741912] = 'enter'
SDL.update({ord(char): char for char in string.ascii_lowercase + string.digits})
SDL.update({1073741882 + i: f'f{i + 1}' for i in range(12)})
SDL.update(dict(zip(range(1073741897, 1073741907),
                    ('insert', 'home', 'page_up', 'delete', 'end', 'page_down', 'right', 'left', 'down', 'up'))))
SDL.update({ord(char): name for char, name in zip('-=[];\'`,./\\',
            ('minus', 'equal', 'left_bracket', 'right_bracket', 'semicolon', 'apostrophe', 'grave', 'comma', 'period', 'slash', 'backslash'))})
QT = {ord(char.upper()): char for char in string.ascii_lowercase + string.digits}
QT.update({code: name for code, name in SDL.items() if code < 128 and not chr(code).isalpha()})
QT.update(dict(zip(range(0x1000010, 0x1000018), ('home', 'end', 'left', 'up', 'right', 'down', 'page_up', 'page_down'))))
QT.update({0x1000030 + i: f'f{i + 1}' for i in range(12)})
QT.update({0x1000000: 'escape', 0x1000001: 'tab', 0x1000003: 'backspace',
           0x1000004: 'enter', 0x1000005: 'enter', 0x1000006: 'insert', 0x1000007: 'delete'})


def binding(code, qt=False):
    modifiers = []
    if qt:
        for mask, name in ((0x2000000, 'shift'), (0x4000000, 'ctrl'), (0x8000000, 'alt'), (0x10000000, 'super')):
            if code & mask:
                modifiers.append(name)
                code &= ~mask
    key = (QT if qt else SDL).get(code)
    return '+'.join(modifiers + [key]) if key else None
