-- ************************************************************
-- Title:	PUBLIC_DVD_ROOFTOP_PV_ACTUAL_201910.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS ROOFTOP_PV_ACTUAL owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_ROOFTOP_PV_ACTUAL_201910010000.CSV
APPEND INTO TABLE ROOFTOP_PV_ACTUAL
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
INTERVAL_DATETIME DATE "yyyy/mm/dd hh24:mi:ss",
REGIONID CHAR(20),
POWER FLOAT EXTERNAL,
QI FLOAT EXTERNAL,
TYPE CHAR(20),
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss")
-- End of Script
