-- ************************************************************
-- Title:	PUBLIC_DVD_APEVENTREGION_202405.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS APEVENTREGION owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_APEVENTREGION_202405010000.CSV
APPEND INTO TABLE APEVENTREGION
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
APEVENTID FLOAT EXTERNAL,
REGIONID CHAR(10),
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss",
ENERGYAPFLAG FLOAT EXTERNAL,
RAISE1SECAPFLAG FLOAT EXTERNAL,
RAISE6SECAPFLAG FLOAT EXTERNAL,
RAISE60SECAPFLAG FLOAT EXTERNAL,
RAISE5MINAPFLAG FLOAT EXTERNAL,
RAISEREGAPFLAG FLOAT EXTERNAL,
LOWER1SECAPFLAG FLOAT EXTERNAL,
LOWER6SECAPFLAG FLOAT EXTERNAL,
LOWER60SECAPFLAG FLOAT EXTERNAL,
LOWER5MINAPFLAG FLOAT EXTERNAL,
LOWERREGAPFLAG FLOAT EXTERNAL)
-- End of Script
