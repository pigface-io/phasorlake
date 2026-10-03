-- ************************************************************
-- Title:	PUBLIC_DVD_RESIDUE_CON_FUNDS_202401.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS RESIDUE_CON_FUNDS owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_RESIDUE_CON_FUNDS_202401010000.CSV
APPEND INTO TABLE RESIDUE_CON_FUNDS
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
CONTRACTID CHAR(30),
INTERCONNECTORID CHAR(10),
FROMREGIONID CHAR(10),
DEFAULTUNITS FLOAT EXTERNAL,
ROLLOVERUNITS FLOAT EXTERNAL,
REALLOCATEDUNITS FLOAT EXTERNAL,
UNITSOFFERED FLOAT EXTERNAL,
MEANRESERVEPRICE FLOAT EXTERNAL,
SCALEFACTOR FLOAT EXTERNAL,
ACTUALRESERVEPRICE FLOAT EXTERNAL,
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss")
-- End of Script
