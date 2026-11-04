// The 80 COCO object classes recognized by the AI providers (labels as sent
// by the server, in English), with their French names, grouped for the settings.

import { lang, t } from './i18n'

export interface ObjectGroup {
  name: string
  labels: string[]
}

const NAMES: Record<string, string> = {
  person: 'Personne',
  bicycle: 'Vélo',
  car: 'Voiture',
  motorcycle: 'Moto',
  airplane: 'Avion',
  bus: 'Bus',
  train: 'Train',
  truck: 'Camion',
  boat: 'Bateau',
  'traffic light': 'Feu tricolore',
  'fire hydrant': 'Bouche d’incendie',
  'stop sign': 'Panneau stop',
  'parking meter': 'Parcmètre',
  bench: 'Banc',
  bird: 'Oiseau',
  cat: 'Chat',
  dog: 'Chien',
  horse: 'Cheval',
  sheep: 'Mouton',
  cow: 'Vache',
  elephant: 'Éléphant',
  bear: 'Ours',
  zebra: 'Zèbre',
  giraffe: 'Girafe',
  backpack: 'Sac à dos',
  umbrella: 'Parapluie',
  handbag: 'Sac à main',
  tie: 'Cravate',
  suitcase: 'Valise',
  frisbee: 'Frisbee',
  skis: 'Skis',
  snowboard: 'Snowboard',
  'sports ball': 'Ballon',
  kite: 'Cerf-volant',
  'baseball bat': 'Batte',
  'baseball glove': 'Gant de baseball',
  skateboard: 'Skateboard',
  surfboard: 'Planche de surf',
  'tennis racket': 'Raquette',
  bottle: 'Bouteille',
  'wine glass': 'Verre à vin',
  cup: 'Tasse',
  fork: 'Fourchette',
  knife: 'Couteau',
  spoon: 'Cuillère',
  bowl: 'Bol',
  banana: 'Banane',
  apple: 'Pomme',
  sandwich: 'Sandwich',
  orange: 'Orange',
  broccoli: 'Brocoli',
  carrot: 'Carotte',
  'hot dog': 'Hot-dog',
  pizza: 'Pizza',
  donut: 'Beignet',
  cake: 'Gâteau',
  chair: 'Chaise',
  couch: 'Canapé',
  'potted plant': 'Plante en pot',
  bed: 'Lit',
  'dining table': 'Table',
  toilet: 'Toilettes',
  tv: 'Télévision',
  laptop: 'Ordinateur portable',
  mouse: 'Souris',
  remote: 'Télécommande',
  keyboard: 'Clavier',
  'cell phone': 'Téléphone',
  microwave: 'Micro-ondes',
  oven: 'Four',
  toaster: 'Grille-pain',
  sink: 'Évier',
  refrigerator: 'Réfrigérateur',
  book: 'Livre',
  clock: 'Horloge',
  vase: 'Vase',
  scissors: 'Ciseaux',
  'teddy bear': 'Ours en peluche',
  'hair drier': 'Sèche-cheveux',
  toothbrush: 'Brosse à dents',
}

/** Groups shown first, then « Autres objets » for the rest. */
export const OBJECT_GROUPS: ObjectGroup[] = [
  { name: t('Personnes', 'People'), labels: ['person'] },
  { name: t('Véhicules', 'Vehicles'), labels: ['car', 'truck', 'bus', 'motorcycle', 'bicycle', 'boat', 'train', 'airplane'] },
  { name: t('Animaux', 'Animals'), labels: ['dog', 'cat', 'bird', 'horse', 'sheep', 'cow', 'bear', 'elephant', 'zebra', 'giraffe'] },
  { name: t('Objets portés', 'Carried objects'), labels: ['backpack', 'handbag', 'suitcase', 'umbrella'] },
]

/** Every label, in model order. */
export const ALL_OBJECTS = Object.keys(NAMES)

/** Labels not in [`OBJECT_GROUPS`]. */
export const OTHER_OBJECTS = ALL_OBJECTS.filter((l) => !OBJECT_GROUPS.some((g) => g.labels.includes(l)))

/** French name of a label (the label itself if unknown). */
export function objectLabel(label: string): string {
  // In English, the label itself (COCO names are English).
  if (lang() === 'en') return label.charAt(0).toUpperCase() + label.slice(1)
  return NAMES[label] ?? label
}
